//! The peak working set of the process, from the operating system.

/// The peak working set of this process so far, in bytes, from the operating
/// system; `None` where this does not read it.
pub(super) fn peak_resident_bytes() -> Option<u64> {
    #[cfg(windows)]
    {
        // a read-only system tool, started by the test itself
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command"])
            .arg(format!(
                "(Get-Process -Id {}).PeakWorkingSet64",
                std::process::id()
            ))
            .output()
            .ok()?;
        parse_count(&String::from_utf8_lossy(&output.stdout))
    }
    #[cfg(target_os = "linux")]
    {
        parse_vm_hwm(&std::fs::read_to_string("/proc/self/status").ok()?)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// The number a command printed on its last line.
fn parse_count(output: &str) -> Option<u64> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .next_back()?
        .parse()
        .ok()
}

/// `VmHWM` of `/proc/<pid>/status`, in bytes: its peak resident set.
fn parse_vm_hwm(status: &str) -> Option<u64> {
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?;
    let kib: u64 = line.trim().strip_suffix("kB")?.trim().parse().ok()?;
    Some(kib * 1024)
}

#[test]
fn the_peak_is_read_from_what_the_system_prints() {
    assert_eq!(parse_count("123456789\r\n"), Some(123_456_789));
    assert_eq!(parse_count("\r\nwarning\r\n42\r\n\r\n"), Some(42));
    assert_eq!(parse_count(""), None);
    assert_eq!(parse_count("not a number"), None);
    let status = "Name:\trurge\nVmPeak:\t  900000 kB\nVmHWM:\t  123456 kB\nVmRSS:\t   100000 kB\n";
    assert_eq!(parse_vm_hwm(status), Some(123_456 * 1024));
    assert_eq!(parse_vm_hwm("Name:\trurge\n"), None);
    assert_eq!(parse_vm_hwm("VmHWM:\tmuch\n"), None);
}
