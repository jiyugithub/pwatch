use clap::Parser;
use colored::Colorize;
use log::error;
use perf::{PerfMap, SampleData};
use perf_event_open_sys as sys;
use std::process;
mod arch;
mod perf;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "0")]
    /// buffer size, in power of 2. For example, 2 means 2^2 pages = 4 * 4096 bytes.
    buf_size: usize,
    #[arg(short)]
    /// whether the target is a thread or a process.
    thread: bool,
    #[arg(short, long)]
    /// whether to print backtrace.
    backtrace: bool,
    /// target pid, if thread is true, this is the tid of the target thread.
    pid: u32,
    /// watchpoint type, can be read(r), write(w), readwrite(rw) or execve(x).
    /// if it is one of r, w, rw, the watchpoint length is needed. Valid length is 1, 2, 4, 8.
    /// For example, r4 means a read watchpoint with length 4 and rw1 means a readwrite watchpoint with length 1.
    r#type: String,
    /// watchpoint address, in hex format. 0x prefix is optional.
    addr: String,
}

fn parse_len(s: &str) -> Option<u32> {
    match s {
        "1" => Some(sys::bindings::HW_BREAKPOINT_LEN_1),
        "2" => Some(sys::bindings::HW_BREAKPOINT_LEN_2),
        "4" => Some(sys::bindings::HW_BREAKPOINT_LEN_4),
        "8" => Some(sys::bindings::HW_BREAKPOINT_LEN_8),
        "" => Some(sys::bindings::HW_BREAKPOINT_LEN_1),
        _ => None,
    }
}

fn parse_watchpoint_type(s: &str) -> Option<(u32, u32)> {
    if let Some(s) = s.strip_prefix("rw") {
        let len = parse_len(s)?;
        Some((sys::bindings::HW_BREAKPOINT_RW, len))
    } else if let Some(s) = s.strip_prefix('r') {
        let len = parse_len(s)?;
        Some((sys::bindings::HW_BREAKPOINT_R, len))
    } else if let Some(s) = s.strip_prefix('w') {
        let len = parse_len(s)?;
        Some((sys::bindings::HW_BREAKPOINT_W, len))
    } else if s == "x" {
        Some((
            sys::bindings::HW_BREAKPOINT_X,
            std::mem::size_of::<nix::libc::c_long>() as u32,
        ))
    } else {
        None
    }
}

fn parse_addr(s: &str) -> Option<u64> {
    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    pretty_env_logger::init();
    let args = Args::parse();

    let (ty, bp_len) = parse_watchpoint_type(&args.r#type)
        .ok_or_else(|| anyhow::anyhow!(format!("invalid watchpoint type: {}", args.r#type)))?;
    let addr = parse_addr(&args.addr)
        .ok_or_else(|| anyhow::anyhow!(format!("invalid address: {}", args.addr)))?;
    let maps = if !args.thread {
        procfs::process::Process::new(args.pid as i32)?
            .tasks()?
            .filter_map(Result::ok)
            .map(|t| {
                PerfMap::new(
                    ty,
                    addr,
                    bp_len as u64,
                    t.tid,
                    args.buf_size,
                    args.backtrace,
                )
            })
            .filter_map(|r| match r {
                Ok(m) => Some(m),
                Err(e) => {
                    error!("perf_map_open error: {}", e);
                    None
                }
            })
            .collect::<Vec<_>>()
    } else {
        vec![PerfMap::new(
            ty,
            addr,
            bp_len as u64,
            args.pid as i32,
            args.buf_size,
            args.backtrace,
        )?]
    };
    if maps.is_empty() {
        error!("no valid perf map");
        return Ok(());
    }
    println!("processPid:{}",process::id());
    let (res, _, _) = futures::future::select_all(maps.into_iter().map(|m| {
        tokio::spawn(async move {
            if let Err(e) = m.events(handle_event).await {
                error!("error: {}", e);
            }
        })
    }))
    .await;
    res?;
    Ok(())
}

fn handle_event(data: SampleData) {
    println!("start");
    // 创建 JSON 字符串的开头
    let mut json_string = String::from("{\"registers\": {");

    for (i, reg) in data.regs.iter().enumerate() {
        // 拼接寄存器值
        json_string.push_str(&format!("\"{}\": \"0x{:016x}\"", arch::id_to_str(i), reg));
        if i < data.regs.len() - 1 {
            json_string.push_str(", "); // 添加逗号分隔
        }
    }
    json_string.push_str("}, \"backtrace\": [");

    if let Some(backtrace) = data.backtrace {
        for (i, addr) in backtrace.iter().enumerate() {
            // 拼接堆栈地址
            json_string.push_str(&format!("\"0x{:016x}\"", addr));
            if i < backtrace.len() - 1 {
                json_string.push_str(", "); // 添加逗号分隔
            }
        }
    }
    json_string.push_str("]}");

    // 输出合并后的 JSON 字符串
    println!("{}", json_string);
    println!("end");
}




