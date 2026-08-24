use log::{Level, Log, Metadata, Record};
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::core::PCWSTR;

pub struct DebugLogger;

impl Log for DebugLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Debug
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let msg = format!("[{}] {}\n", record.level(), record.args());
        send_to_debugger(&msg);
    }

    fn flush(&self) {}
}

fn send_to_debugger(msg: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    let wide: Vec<u16> = OsStr::new(msg)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        OutputDebugStringW(PCWSTR(wide.as_ptr()));
    }
}

pub static LOGGER: DebugLogger = DebugLogger;
