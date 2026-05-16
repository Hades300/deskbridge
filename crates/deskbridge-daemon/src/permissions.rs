use anyhow::{Result, bail};
use std::time::Duration;

pub fn run(prompt: bool) -> Result<()> {
    println!("DeskBridge permissions");
    println!("process: {}", std::env::current_exe()?.display());

    if input_injection_trusted(prompt) {
        println!("accessibility: ok");
        return Ok(());
    }

    println!("accessibility: missing");
    println!("Grant Accessibility to the process shown above in System Settings.");
    if prompt {
        std::thread::sleep(Duration::from_secs(2));
    }
    bail!("accessibility permission is required for keyboard and mouse injection")
}

fn input_injection_trusted(prompt: bool) -> bool {
    let settings = enigo::Settings {
        open_prompt_to_get_permissions: prompt,
        ..Default::default()
    };
    enigo::Enigo::new(&settings).is_ok()
}
