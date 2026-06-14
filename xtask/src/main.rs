use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("dist") => dist(),
        _ => {
            eprintln!("Usage: cargo run -p xtask -- dist");
            Ok(())
        }
    }
}

fn dist() -> Result<()> {
    if cfg!(target_os = "macos") && !cfg!(target_arch = "aarch64") {
        return Err(
            "Intel Mac is not supported. macOS packaging is only available for Apple Silicon."
                .to_string(),
        );
    }

    let root = workspace_root()?;
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let status = Command::new(cargo)
        .current_dir(&root)
        .args(["build", "--release", "-p", "stata-ai-skill"])
        .status()
        .map_err(|err| format!("failed to run cargo build: {err}"))?;
    if !status.success() {
        return Err(format!("cargo build failed with status {status}"));
    }

    let (platform_dir, exe_name) = packaged_binary_name();
    let source = root.join("target").join("release").join(exe_name);
    let dest_dir = root.join("skill").join("bin").join(platform_dir);
    let dest = dest_dir.join(exe_name);

    fs::create_dir_all(&dest_dir)
        .map_err(|err| format!("failed to create {}: {err}", dest_dir.display()))?;
    fs::copy(&source, &dest).map_err(|err| {
        format!(
            "failed to copy {} to {}: {err}",
            source.display(),
            dest.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&dest)
            .map_err(|err| format!("failed to stat {}: {err}", dest.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&dest, permissions)
            .map_err(|err| format!("failed to chmod {}: {err}", dest.display()))?;
    }

    println!("Packaged {}", dest.display());
    Ok(())
}

fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map_err(|err| format!("CARGO_MANIFEST_DIR is not set: {err}"))?;
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("cannot find workspace root from {}", manifest_dir.display()))
}

fn packaged_binary_name() -> (&'static str, &'static str) {
    if cfg!(target_os = "windows") {
        if cfg!(target_arch = "aarch64") {
            ("windows-arm64", "stata-ai-skill.exe")
        } else {
            ("windows", "stata-ai-skill.exe")
        }
    } else if cfg!(target_os = "macos") {
        ("macos-arm64", "stata-ai-skill")
    } else {
        ("unix", "stata-ai-skill")
    }
}
