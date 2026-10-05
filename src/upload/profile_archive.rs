use base64::{Engine, engine::general_purpose};

use crate::prelude::*;
use crate::upload::interfaces::ProfileMultipart;
use bytes::Bytes;
use std::io::{Read, SeekFrom};
use std::path::PathBuf;
use std::sync::LazyLock;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Default number of multipart upload parts sent at the same time
///
/// Measured on `ubuntu:latest` and `macro runner gen 2` with a sweep from 1, 2, 4, 8, 16
/// After 8 no more performance improvement was observed.
const DEFAULT_CONCURRENT_PART_UPLOADS: usize = 8;

/// Overrides [`DEFAULT_CONCURRENT_PART_UPLOADS`]
const CONCURRENT_PART_UPLOADS_ENV: &str = "CODSPEED_UPLOAD_CONCURRENCY";

static CONCURRENT_PART_UPLOADS: LazyLock<usize> = LazyLock::new(|| {
    let Ok(value) = std::env::var(CONCURRENT_PART_UPLOADS_ENV) else {
        return DEFAULT_CONCURRENT_PART_UPLOADS;
    };
    match value.parse::<usize>() {
        Ok(concurrency) if concurrency > 0 => concurrency,
        _ => {
            warn!(
                "Ignoring invalid {CONCURRENT_PART_UPLOADS_ENV} value {value:?}, using {DEFAULT_CONCURRENT_PART_UPLOADS}"
            );
            DEFAULT_CONCURRENT_PART_UPLOADS
        }
    }
});

pub(super) fn concurrent_part_uploads() -> usize {
    *CONCURRENT_PART_UPLOADS
}

const MULTIPART_MIN_PART_SIZE_BYTES: u64 = 16 * 1024 * 1024; // 16 MiB
/// Bounds how much a failed part has to re-send
const MULTIPART_MAX_PART_SIZE_BYTES: u64 = 256 * 1024 * 1024; // 256 MiB
/// More parts than concurrent uploads, so that the upload slots freed by fast parts
/// pick up remaining work instead of idling while the slowest part finishes
const PARTS_PER_CONCURRENT_UPLOAD: u64 = 2;
const HASH_READ_BUFFER_SIZE: usize = 8 * 1024 * 1024; // 8 MiB

#[derive(Debug)]
pub struct ProfileArchive {
    pub md5: String,
    pub content: ProfileArchiveContent,
    pub multipart: ProfileMultipart,
}

#[derive(Debug)]
pub enum ProfileArchiveContent {
    CompressedInMemory { data: Bytes },
    UncompressedOnDisk { path: PathBuf },
}

fn encode_md5(digest: md5::Digest) -> String {
    general_purpose::STANDARD.encode(digest.0)
}

/// Read the content once to compute the md5 of all of it and of each consecutive
/// `part_size` chunk of it.
///
/// CPU heavy + potentially reading from blocking IO, so it is recommended to
/// run this on a blocking thread pool.
fn compute_md5s_from_reader(
    mut reader: impl Read,
    part_size: u64,
) -> Result<(String, Vec<String>)> {
    let mut buffer = vec![0u8; HASH_READ_BUFFER_SIZE];
    let mut whole_context = md5::Context::new();
    let mut part_context = md5::Context::new();
    let mut part_length = 0u64;
    let mut part_md5s = Vec::new();

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let mut chunk = &buffer[..read];
        whole_context.consume(chunk);

        while !chunk.is_empty() {
            let taken = (part_size - part_length).min(chunk.len() as u64) as usize;
            part_context.consume(&chunk[..taken]);
            part_length += taken as u64;
            chunk = &chunk[taken..];
            if part_length == part_size {
                let finished_part = std::mem::replace(&mut part_context, md5::Context::new());
                part_md5s.push(encode_md5(finished_part.finalize()));
                part_length = 0;
            }
        }
    }
    if part_length > 0 {
        part_md5s.push(encode_md5(part_context.finalize()));
    }

    Ok((encode_md5(whole_context.finalize()), part_md5s))
}

/// [`compute_md5s_from_reader`] over the content, run on the blocking thread pool as
/// hashing the whole archive would otherwise stall the async runtime.
async fn compute_md5s(
    content: &ProfileArchiveContent,
    part_size: u64,
) -> Result<(String, Vec<String>)> {
    match content {
        ProfileArchiveContent::CompressedInMemory { data } => {
            let data = data.clone();
            tokio::task::spawn_blocking(move || compute_md5s_from_reader(&data[..], part_size))
                .await?
        }
        ProfileArchiveContent::UncompressedOnDisk { path } => {
            let path = path.clone();
            tokio::task::spawn_blocking(move || {
                compute_md5s_from_reader(std::fs::File::open(path)?, part_size)
            })
            .await?
        }
    }
}

/// Split the archive in about `PARTS_PER_CONCURRENT_UPLOAD` parts per concurrent upload,
/// within the part size bounds. An archive no larger than the minimum part size is a
/// single part.
fn choose_multipart_part_size(size: u64, concurrent_uploads: usize) -> u64 {
    let target_part_count = concurrent_uploads as u64 * PARTS_PER_CONCURRENT_UPLOAD;
    size.div_ceil(target_part_count)
        .clamp(MULTIPART_MIN_PART_SIZE_BYTES, MULTIPART_MAX_PART_SIZE_BYTES)
}

impl ProfileArchive {
    pub async fn new_compressed_in_memory(data: Vec<u8>) -> Result<Self> {
        Self::new(ProfileArchiveContent::CompressedInMemory { data: data.into() }).await
    }

    pub async fn new_uncompressed_on_disk(path: PathBuf) -> Result<Self> {
        let metadata = tokio::fs::metadata(&path).await?;
        if !metadata.is_file() {
            return Err(anyhow!("The provided path is not a file"));
        }
        Self::new(ProfileArchiveContent::UncompressedOnDisk { path }).await
    }

    async fn new(content: ProfileArchiveContent) -> Result<Self> {
        let size = content.size().await?;
        let part_size = choose_multipart_part_size(size, concurrent_part_uploads());

        let (md5, part_md5s) = compute_md5s(&content, part_size).await?;
        let multipart = ProfileMultipart {
            size,
            part_size,
            part_md5s,
        };

        Ok(ProfileArchive {
            md5,
            content,
            multipart,
        })
    }
}

impl ProfileArchiveContent {
    pub async fn size(&self) -> Result<u64> {
        match &self {
            ProfileArchiveContent::CompressedInMemory { data } => Ok(data.len() as u64),
            ProfileArchiveContent::UncompressedOnDisk { path } => {
                let metadata = tokio::fs::metadata(path).await?;
                Ok(metadata.len())
            }
        }
    }

    pub fn encoding(&self) -> Option<String> {
        match self {
            ProfileArchiveContent::CompressedInMemory { .. } => Some("gzip".to_string()),
            ProfileArchiveContent::UncompressedOnDisk { .. } => None,
        }
    }

    /// Request body holding `length` bytes of the content starting at `offset`.
    /// On-disk content is streamed rather than loaded in memory.
    pub async fn body(&self, offset: u64, length: u64) -> Result<reqwest::Body> {
        match self {
            ProfileArchiveContent::CompressedInMemory { data } => {
                let start = offset as usize;
                Ok(data.slice(start..start + length as usize).into())
            }
            ProfileArchiveContent::UncompressedOnDisk { path } => {
                let mut file = tokio::fs::File::open(path)
                    .await
                    .context(format!("Failed to open file at path: {}", path.display()))?;
                file.seek(SeekFrom::Start(offset)).await?;
                let stream = tokio_util::io::ReaderStream::new(file.take(length));
                Ok(reqwest::Body::wrap_stream(stream))
            }
        }
    }
}

impl Drop for ProfileArchiveContent {
    fn drop(&mut self) {
        if let ProfileArchiveContent::UncompressedOnDisk { path } = self {
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_file(content: &[u8]) -> PathBuf {
        let path = tempfile::NamedTempFile::new()
            .unwrap()
            .into_temp_path()
            .keep()
            .unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn computes_whole_and_part_md5s_in_one_pass() {
        // Not a multiple of the part size, and spanning several read buffers
        let content: Vec<u8> = (0..HASH_READ_BUFFER_SIZE * 2 + 123)
            .map(|i| (i % 251) as u8)
            .collect();
        let part_size = (HASH_READ_BUFFER_SIZE / 3) as u64;

        let (hash, part_md5s) = compute_md5s_from_reader(&content[..], part_size).unwrap();

        assert_eq!(hash, encode_md5(md5::compute(&content)));
        let expected_part_md5s: Vec<String> = content
            .chunks(part_size as usize)
            .map(|part| encode_md5(md5::compute(part)))
            .collect();
        assert_eq!(part_md5s, expected_part_md5s);
    }

    #[tokio::test]
    async fn small_archive_is_a_single_part() {
        let path = write_temp_file(b"profile-archive");

        let archive = ProfileArchive::new_uncompressed_on_disk(path)
            .await
            .unwrap();

        let md5 = encode_md5(md5::compute(b"profile-archive"));
        assert_eq!(archive.md5, md5);
        assert_eq!(
            archive.multipart,
            ProfileMultipart {
                size: b"profile-archive".len() as u64,
                part_size: MULTIPART_MIN_PART_SIZE_BYTES,
                part_md5s: vec![md5],
            }
        );
    }

    #[test]
    fn archive_is_split_in_two_parts_per_concurrent_upload() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(choose_multipart_part_size(1024 * MIB, 8), 64 * MIB);
    }

    #[test]
    fn part_size_is_clamped() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(
            choose_multipart_part_size(64 * MIB, 8),
            MULTIPART_MIN_PART_SIZE_BYTES
        );
        assert_eq!(
            choose_multipart_part_size(15 * 1024 * MIB, 8),
            MULTIPART_MAX_PART_SIZE_BYTES
        );
    }
}
