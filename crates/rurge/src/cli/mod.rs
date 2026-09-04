pub mod check;
pub mod rule;
pub mod runtime;

use rurge_config::config::Platform;
use rurge_config::requirement::Environment;

pub fn environment(platform: Platform, core_version: u64) -> Environment {
    let language = std::env::var("LANG")
        .ok()
        .and_then(|l| l.split('.').next().map(|s| s.replace('_', "-")))
        .filter(|l| !l.is_empty() && l != "C")
        .unwrap_or_else(|| "en-US".to_string());
    let device_name = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "rurge".to_string());
    Environment {
        core_version,
        system: platform.system_name().to_string(),
        system_version: "unknown".to_string(),
        device_model: std::env::consts::ARCH.to_string(),
        language,
        device_name,
    }
}
