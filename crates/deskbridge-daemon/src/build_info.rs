pub fn version() -> &'static str {
    option_env!("DESKBRIDGE_BUILD_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub fn commit() -> Option<&'static str> {
    option_env!("DESKBRIDGE_BUILD_COMMIT").filter(|value| !value.is_empty())
}

pub fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

pub fn lines() -> Vec<String> {
    let mut lines = vec![
        format!("version={}", version()),
        format!("protocol={}", deskbridge_core::PROTOCOL_VERSION),
        format!("platform={}", platform()),
    ];
    if let Some(commit) = commit() {
        lines.push(format!("commit={commit}"));
    }
    lines
}
