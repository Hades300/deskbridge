use anyhow::{Result, bail};

pub fn run(prompt: bool) -> Result<()> {
    println!("DeskBridge permissions");
    println!("process: {}", std::env::current_exe()?.display());

    if accessibility_trusted(prompt) {
        println!("accessibility: ok");
        return Ok(());
    }

    println!("accessibility: missing");
    println!("Grant Accessibility to the process shown above in System Settings.");
    bail!("accessibility permission is required for keyboard and mouse injection")
}

#[cfg(target_os = "macos")]
fn accessibility_trusted(prompt: bool) -> bool {
    unsafe {
        if prompt {
            CGRequestPostEventAccess()
        } else {
            CGPreflightPostEventAccess()
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn accessibility_trusted(_prompt: bool) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}
