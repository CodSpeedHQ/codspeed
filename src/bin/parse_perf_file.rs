use codspeed_runner::executor::wall_time::perf::parse_perf_file::{self, PidFilter};

fn main() {
    codspeed_runner::init_local_logger().unwrap();
    let perf_file_path = "/home/guillaume/cod-2314/profile.pPMUwlf7Pu.out/perf.pipedata";
    let pid_filter = PidFilter::All;
    parse_perf_file::parse_for_memmap2(perf_file_path, pid_filter).unwrap_or_else(|e| {
        eprintln!("Failed to parse perf file: {e}");
        std::process::exit(1);
    });
    println!("Parsed successfully");
}
