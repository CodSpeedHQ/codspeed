/// Run `$body` with `$skel` bound to the loaded skeleton. The two variants are
/// distinct generated types, so this is a match rather than a function, but they
/// share field and method names for all maps and auto-attached programs.
macro_rules! with_skel {
    ($self:expr, $skel:ident => $body:expr) => {
        match &$self.skel {
            crate::ebpf::memtrack::Skel::Token($skel) => $body,
            crate::ebpf::memtrack::Skel::Legacy($skel) => $body,
        }
    };
    (mut $self:expr, $skel:ident => $body:expr) => {
        match &mut $self.skel {
            crate::ebpf::memtrack::Skel::Token($skel) => $body,
            crate::ebpf::memtrack::Skel::Legacy($skel) => $body,
        }
    };
}

/// Attach one program (entry or return) at `offset` in `lib_path`, through the
/// mechanism the loaded variant was built for.
macro_rules! attach_one {
    ($self:expr, $prog:ident, $lib_path:expr, $offset:expr, $retprobe:expr) => {{
        let lib_path = $lib_path;
        let offset = $offset;
        let retprobe = $retprobe;
        match &mut $self.skel {
            crate::ebpf::memtrack::Skel::Token(skel) => {
                skel.progs.$prog.attach_uprobe_multi_with_opts(
                    -1,
                    lib_path,
                    "",
                    UprobeMultiOpts {
                        offsets: vec![offset],
                        retprobe,
                        ..Default::default()
                    },
                )
            }
            crate::ebpf::memtrack::Skel::Legacy(skel) => skel.progs.$prog.attach_uprobe_with_opts(
                -1,
                lib_path,
                offset,
                UprobeOpts {
                    retprobe,
                    ..Default::default()
                },
            ),
        }
    }};
}

/// Macro to attach a function with both entry and return probes at a resolved
/// file offset. Also generates an `attach_*_if_found` variant that skips
/// symbols absent from the offset table (returning whether it attached) and
/// propagates attach failures.
macro_rules! attach_uprobe_uretprobe {
    ($name:ident, $prog_entry:ident, $prog_return:ident) => {
        paste! {
            fn [<try_ $name>](&mut self, lib_path: &Path, offset: usize) -> Result<()> {
                let link = attach_one!(self, $prog_entry, lib_path, offset, false)
                    .context(format!(
                        "Failed to attach uprobe at offset {:#x} in {}",
                        offset,
                        lib_path.display()
                    ))?;
                self.probes.push(link);

                let link = attach_one!(self, $prog_return, lib_path, offset, true)
                    .context(format!(
                        "Failed to attach uretprobe at offset {:#x} in {}",
                        offset,
                        lib_path.display()
                    ))?;
                self.probes.push(link);

                Ok(())
            }

            fn [<$name _if_found>](
                &mut self,
                lib_path: &Path,
                symbol: &str,
                symbols: &ResolvedSymbols,
            ) -> Result<bool> {
                let Some(offset) = symbols.offset(symbol) else {
                    return Ok(false);
                };
                self.[<try_ $name>](lib_path, offset)
                    .with_context(|| format!("Failed to attach {symbol}"))?;
                log::trace!("Attached {} at {:#x}", symbol, offset);
                Ok(true)
            }
        }
    };
}

/// Macro to attach a function with only an entry probe (no return probe) at a
/// resolved file offset. Also generates an `attach_*_if_found` variant that
/// skips symbols absent from the offset table (returning whether it attached)
/// and propagates attach failures.
macro_rules! attach_uprobe {
    ($name:ident, $prog:ident) => {
        paste! {
            fn [<try_ $name>](&mut self, lib_path: &Path, offset: usize) -> Result<()> {
                let link = attach_one!(self, $prog, lib_path, offset, false)
                    .context(format!(
                        "Failed to attach uprobe at offset {:#x} in {}",
                        offset,
                        lib_path.display()
                    ))?;
                self.probes.push(link);
                Ok(())
            }

            fn [<$name _if_found>](
                &mut self,
                lib_path: &Path,
                symbol: &str,
                symbols: &ResolvedSymbols,
            ) -> Result<bool> {
                let Some(offset) = symbols.offset(symbol) else {
                    return Ok(false);
                };
                self.[<try_ $name>](lib_path, offset)
                    .with_context(|| format!("Failed to attach {symbol}"))?;
                log::trace!("Attached {} at {:#x}", symbol, offset);
                Ok(true)
            }
        }
    };
}

macro_rules! attach_tracepoint {
    ($func:ident, $prog:ident) => {
        fn $func(&mut self) -> Result<()> {
            let link = with_skel!(mut self, skel => skel.progs.$prog.attach())
                .context(format!("Failed to attach {} tracepoint", stringify!($prog)))?;
            self.probes.push(link);
            Ok(())
        }
    };
    ($name:ident) => {
        paste! {
            attach_tracepoint!([<attach_ $name>], [<tracepoint_ $name>]);
        }
    };
}
