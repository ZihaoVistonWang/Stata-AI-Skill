use std::env;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use encoding_rs::{BIG5, EUC_KR, GBK, SHIFT_JIS, WINDOWS_1252};
use serde_json::{json, Value};

const SERVICE_NAME: &str = "stata-ai-skill";
const SKILL_VERSION: &str = "202607060001";
const DEFAULT_PORT: u16 = 19522;
const DEFAULT_TIMEOUT_SEC: u64 = 30;
const MAX_TIMEOUT_SEC: u64 = 600;
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const SETUP_TOKEN_TTL: Duration = Duration::from_secs(600);
const SETUP_PROTOCOL_VERSION: u8 = 1;
static UNIQUE_COUNTER: AtomicU64 = AtomicU64::new(0);

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        return Err(
            "Stata AI Skill native service does not support Intel Mac. This skill currently supports Apple Silicon macOS and Windows x64/ARM64."
                .to_string(),
        );
    }

    let args: Vec<String> = env::args().collect();
    let paths = AppPaths::new()?;
    paths.ensure()?;

    match args.get(1).map(String::as_str) {
        Some("serve") => {
            let cli_stata_path = arg_value(&args, "--stata-path").map(PathBuf::from);
            let cli_port = arg_value(&args, "--port").and_then(|v| v.parse::<u16>().ok());
            let mut config = AppConfig::load(&paths).unwrap_or_default();
            if let Some(path) = cli_stata_path {
                config.stata_path = Some(path);
            }
            if let Some(port) = cli_port {
                config.port = port;
            }
            serve(config, paths)
        }
        Some("config") if args.get(2).map(String::as_str) == Some("set") => {
            let mut config = AppConfig::load(&paths).unwrap_or_default();
            if let Some(path) = arg_value(&args, "--stata-path") {
                config.stata_path = Some(PathBuf::from(path));
            }
            if let Some(port) = arg_value(&args, "--port").and_then(|v| v.parse::<u16>().ok()) {
                config.port = port;
            }
            config.save(&paths)?;
            println!("{}", config.to_toml());
            Ok(())
        }
        Some("config") if args.get(2).map(String::as_str) == Some("show") => {
            let config = AppConfig::load(&paths).unwrap_or_default();
            println!("{}", config.to_toml());
            Ok(())
        }
        Some("config") if args.get(2).map(String::as_str) == Some("reset") => {
            remove_persisted_config(&paths)?;
            println!("Stata AI Skill configuration reset.");
            Ok(())
        }
        _ => {
            eprintln!(
                "Usage:\n  stata-ai-skill serve [--stata-path <path>] [--port <port>]\n  stata-ai-skill config set [--stata-path <path>] [--port <port>]\n  stata-ai-skill config show\n  stata-ai-skill config reset"
            );
            Ok(())
        }
    }
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.windows(2)
        .find(|pair| pair[0] == key)
        .map(|pair| pair[1].clone())
}

#[derive(Clone, Debug)]
struct AppPaths {
    config_dir: PathBuf,
    config_file: PathBuf,
    log_dir: PathBuf,
    temp_dir: PathBuf,
    graph_dir: PathBuf,
}

impl AppPaths {
    fn new() -> Result<Self> {
        let home = home_dir()?;
        #[cfg(target_os = "macos")]
        {
            let config_dir = home
                .join("Library")
                .join("Application Support")
                .join("stata-ai-skill");
            return Ok(Self {
                config_file: config_dir.join("config.toml"),
                log_dir: home.join("Library").join("Logs").join("stata-ai-skill"),
                temp_dir: env::temp_dir().join("stata-ai-skill"),
                graph_dir: config_dir.join("graphs"),
                config_dir,
            });
        }

        #[cfg(target_os = "windows")]
        {
            let appdata = env::var_os("APPDATA")
                .map(PathBuf::from)
                .ok_or_else(|| "APPDATA is not set".to_string())?;
            let local = env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| appdata.clone());
            let config_dir = appdata.join("StataAISkill");
            return Ok(Self {
                config_file: config_dir.join("config.toml"),
                log_dir: local.join("StataAISkill").join("Logs"),
                temp_dir: env::temp_dir().join("StataAISkill"),
                graph_dir: local.join("StataAISkill").join("Graphs"),
                config_dir,
            });
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let config_dir = home.join(".config").join("stata-ai-skill");
            return Ok(Self {
                config_file: config_dir.join("config.toml"),
                log_dir: home.join(".local").join("state").join("stata-ai-skill"),
                temp_dir: env::temp_dir().join("stata-ai-skill"),
                graph_dir: home
                    .join(".local")
                    .join("share")
                    .join("stata-ai-skill")
                    .join("graphs"),
                config_dir,
            });
        }
    }

    fn ensure(&self) -> Result<()> {
        for dir in [
            &self.config_dir,
            &self.log_dir,
            &self.temp_dir,
            &self.graph_dir,
        ] {
            fs::create_dir_all(dir).map_err(|err| format!("failed to create {dir:?}: {err}"))?;
        }
        Ok(())
    }
}

fn home_dir() -> Result<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "cannot locate user home directory".to_string())
}

#[derive(Clone, Debug)]
struct AppConfig {
    port: u16,
    stata_path: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            stata_path: None,
        }
    }
}

impl AppConfig {
    fn load(paths: &AppPaths) -> Result<Self> {
        let text = fs::read_to_string(&paths.config_file)
            .map_err(|err| format!("failed to read config: {err}"))?;
        let mut config = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("port") {
                if let Some(value) = line.split('=').nth(1) {
                    if let Ok(port) = value.trim().parse::<u16>() {
                        config.port = port;
                    }
                }
            } else if line.starts_with("stata_path") {
                if let Some(value) = parse_quoted_value(line) {
                    config.stata_path = Some(PathBuf::from(value));
                }
            }
        }
        Ok(config)
    }

    fn save(&self, paths: &AppPaths) -> Result<()> {
        paths.ensure()?;
        fs::write(&paths.config_file, self.to_toml())
            .map_err(|err| format!("failed to write config: {err}"))
    }

    fn to_toml(&self) -> String {
        let mut out = format!("port = {}\n", self.port);
        if let Some(path) = &self.stata_path {
            out.push_str(&format!(
                "stata_path = \"{}\"\n",
                path.to_string_lossy()
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
            ));
        }
        out
    }
}

fn remove_persisted_config(paths: &AppPaths) -> Result<()> {
    match fs::remove_file(&paths.config_file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "failed to remove {}: {error}",
            paths.config_file.display()
        )),
    }
}

fn parse_quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let end = line[start..].rfind('"')? + start;
    Some(line[start..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

#[derive(Clone, Debug)]
struct DiscoveryCandidate {
    display_name: String,
    selected_path: PathBuf,
    library_path: PathBuf,
    license_path: Option<PathBuf>,
    license_found: bool,
    edition: String,
    version: Option<u32>,
    source: String,
}

impl DiscoveryCandidate {
    fn value(&self, recommended: bool) -> Value {
        json!({
            "displayName": self.display_name,
            "path": self.selected_path.to_string_lossy(),
            "libraryPath": self.library_path.to_string_lossy(),
            "licensePath": self.license_path.as_ref().map(|path| path.to_string_lossy().to_string()),
            "licenseFound": self.license_found,
            "edition": self.edition,
            "version": self.version,
            "source": self.source,
            "recommended": recommended
        })
    }
}

#[derive(Clone, Debug)]
struct Discovery {
    library_path: Option<PathBuf>,
    license_path: Option<PathBuf>,
    license_found: bool,
    needs_configuration: bool,
    needs_license: bool,
    message: String,
    examples: Vec<String>,
    candidates: Vec<DiscoveryCandidate>,
    error: Option<String>,
}

fn discover_stata(configured: Option<&Path>) -> Discovery {
    let configured_candidates = configured.map(resolve_from_user_path).unwrap_or_default();
    let configured_candidate = configured_candidates.first().cloned();
    let (mut candidates, discovery_error) = scan_installations();
    sort_candidates(&mut candidates);
    deduplicate_candidates(&mut candidates);
    let library_path = configured_candidate
        .as_ref()
        .map(|candidate| candidate.library_path.clone());
    let license_path = library_path.as_deref().and_then(expected_license_path);
    let license_found = license_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    let needs_configuration = configured_candidate.is_none();
    let needs_license = library_path.is_some() && !license_found;
    Discovery {
        library_path,
        license_path,
        license_found,
        needs_configuration,
        needs_license,
        message: if needs_configuration && !candidates.is_empty() {
            "Stata installations were detected. Ask the user which installation to use before configuring the service.".to_string()
        } else if needs_configuration {
            format!(
                "Stata was not found automatically. Create an aiskill installation session, give the user the generated installation.do command, and wait for the selected GUI Stata to report the result. Manual path examples: {}.",
                example_paths().join(" or ")
            )
        } else if needs_license {
            "Stata was found, but the license file stata.lic / STATA.lic was not found in the expected Stata installation directory. Ask the user to confirm Stata is licensed and that the license file exists next to the Stata installation.".to_string()
        } else {
            "Stata library and license file found.".to_string()
        },
        examples: example_paths(),
        candidates,
        error: discovery_error,
    }
}

fn resolve_from_user_path(path: &Path) -> Vec<DiscoveryCandidate> {
    if is_library_path(path) {
        return vec![candidate_from_library(
            path.to_path_buf(),
            path.to_path_buf(),
            "configured",
        )];
    }
    #[cfg(target_os = "macos")]
    {
        if path.extension().and_then(|v| v.to_str()) == Some("app") {
            return macos_candidates_for_app(path, "configured");
        }
        if path.is_dir() {
            let direct = macos_libraries_in(path)
                .into_iter()
                .map(|library| candidate_from_library(path.to_path_buf(), library, "configured"))
                .collect::<Vec<_>>();
            if !direct.is_empty() {
                return direct;
            }
            if let Ok(entries) = fs::read_dir(path) {
                return entries
                    .flatten()
                    .flat_map(|entry| macos_candidates_for_app(&entry.path(), "configured"))
                    .collect();
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if path.is_file() {
            if let Some(parent) = path.parent() {
                return windows_libraries_in(parent)
                    .into_iter()
                    .map(|library| {
                        candidate_from_library(path.to_path_buf(), library, "configured")
                    })
                    .collect();
            }
        }
        if path.is_dir() {
            return windows_libraries_in(path)
                .into_iter()
                .map(|library| candidate_from_library(path.to_path_buf(), library, "configured"))
                .collect();
        }
    }
    Vec::new()
}

fn candidate_from_library(
    selected_path: PathBuf,
    library_path: PathBuf,
    source: &str,
) -> DiscoveryCandidate {
    let license_path = expected_license_path(&library_path);
    let edition = edition_from_library(&library_path);
    DiscoveryCandidate {
        display_name: candidate_display_name(&selected_path, None, &edition),
        selected_path,
        library_path,
        license_found: license_path
            .as_ref()
            .map(|path| path.exists())
            .unwrap_or(false),
        license_path,
        edition,
        version: None,
        source: source.to_string(),
    }
}

fn edition_from_library(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    for edition in ["mp", "se", "be", "ic"] {
        if name.contains(&format!("-{edition}."))
            || name.starts_with(&format!("{edition}-"))
            || name.contains(&format!("stata{edition}"))
        {
            return edition.to_string();
        }
    }
    String::new()
}

fn parse_numeric_version(values: &[&str]) -> Option<u32> {
    for value in values {
        let lower = value.to_ascii_lowercase();
        if let Some(index) = lower.find("statanow").or_else(|| lower.find("stata")) {
            let digits = lower[index..]
                .chars()
                .skip_while(|ch| !ch.is_ascii_digit())
                .take_while(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if let Ok(version) = digits.parse::<u32>() {
                return Some(version);
            }
        }
    }
    None
}

fn candidate_display_name(path: &Path, version: Option<u32>, edition: &str) -> String {
    let base = if let Some(version) = version {
        format!("Stata {version}")
    } else {
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Stata")
            .to_string()
    };
    if edition.is_empty() || base.to_ascii_lowercase().ends_with(edition) {
        base
    } else {
        format!("{base} {}", edition.to_ascii_uppercase())
    }
}

fn edition_rank(edition: &str) -> usize {
    ["mp", "se", "be", "ic"]
        .iter()
        .position(|value| *value == edition)
        .unwrap_or(99)
}

fn sort_candidates(candidates: &mut [DiscoveryCandidate]) {
    candidates.sort_by(|left, right| {
        right
            .version
            .unwrap_or(0)
            .cmp(&left.version.unwrap_or(0))
            .then_with(|| edition_rank(&left.edition).cmp(&edition_rank(&right.edition)))
            .then_with(|| left.selected_path.cmp(&right.selected_path))
    });
}

fn deduplicate_candidates(candidates: &mut Vec<DiscoveryCandidate>) {
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| {
        let key = candidate
            .library_path
            .to_string_lossy()
            .to_ascii_lowercase();
        seen.insert(key)
    });
}

fn is_library_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    #[cfg(target_os = "macos")]
    {
        name.starts_with("libstata-") && name.ends_with(".dylib")
    }
    #[cfg(target_os = "windows")]
    {
        name.ends_with("-64.dll") || (name.starts_with("stata") && name.ends_with(".dll"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        name.ends_with(".so") || name.ends_with(".dylib") || name.ends_with(".dll")
    }
}

#[cfg(target_os = "macos")]
fn scan_installations() -> (Vec<DiscoveryCandidate>, Option<String>) {
    let base = Path::new("/Applications");
    let preferred = ["StataMP", "StataSE", "StataIC", "StataBE", "Stata"];
    let mut apps = Vec::new();

    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if is_stata_app_path(&path) {
                apps.push(path);
                continue;
            }

            if path.is_dir() {
                if let Ok(sub_entries) = fs::read_dir(&path) {
                    for sub_entry in sub_entries.flatten() {
                        let sub_path = sub_entry.path();
                        if is_stata_app_path(&sub_path) {
                            apps.push(sub_path);
                        }
                    }
                }
            }
        }
    }

    apps.sort_by(|a, b| {
        let a_name = app_name_without_ext(a);
        let b_name = app_name_without_ext(b);
        let a_score = preferred
            .iter()
            .position(|name| *name == a_name)
            .unwrap_or(usize::MAX);
        let b_score = preferred
            .iter()
            .position(|name| *name == b_name)
            .unwrap_or(usize::MAX);
        a_score.cmp(&b_score).then_with(|| a_name.cmp(&b_name))
    });

    let mut candidates = apps
        .iter()
        .flat_map(|app| macos_candidates_for_app(app, "applications"))
        .collect::<Vec<_>>();
    sort_candidates(&mut candidates);
    (candidates, None)
}

#[cfg(target_os = "macos")]
fn macos_candidates_for_app(app: &Path, source: &str) -> Vec<DiscoveryCandidate> {
    if !is_stata_app_path(app) {
        return Vec::new();
    }
    let parent_name = app
        .parent()
        .and_then(|value| value.file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("");
    let app_name = app_name_without_ext(app);
    let version = parse_numeric_version(&[parent_name, &app_name]);
    macos_libraries_in(&app.join("Contents").join("MacOS"))
        .into_iter()
        .map(|library| {
            let mut candidate = candidate_from_library(app.to_path_buf(), library, source);
            candidate.version = version;
            candidate.display_name = candidate_display_name(app, version, &candidate.edition);
            candidate
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn macos_libraries_in(dir: &Path) -> Vec<PathBuf> {
    ["mp", "se", "be", "ic"]
        .iter()
        .map(|edition| dir.join(format!("libstata-{edition}.dylib")))
        .filter(|p| p.exists())
        .collect()
}

#[cfg(target_os = "macos")]
fn is_stata_app_path(path: &Path) -> bool {
    path.extension().and_then(|v| v.to_str()) == Some("app")
        && app_name_without_ext(path)
            .to_ascii_lowercase()
            .contains("stata")
}

#[cfg(target_os = "macos")]
fn app_name_without_ext(path: &Path) -> String {
    path.file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(target_os = "windows")]
fn scan_installations() -> (Vec<DiscoveryCandidate>, Option<String>) {
    match run_windows_discovery() {
        Ok(value) => match parse_windows_discovery_report(&value) {
            Ok(candidates) => (candidates, None),
            Err(error) => (Vec::new(), Some(error)),
        },
        Err(error) => (Vec::new(), Some(error)),
    }
}

#[cfg(target_os = "windows")]
fn discovery_script_path() -> Result<PathBuf> {
    if let Some(value) = env::var_os("STATA_AI_SKILL_DISCOVERY_BAT") {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
    }
    let mut candidates = Vec::new();
    if let Ok(executable) = env::current_exe() {
        if let Some(skill_root) = executable
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
        {
            candidates.push(
                skill_root
                    .join("scripts")
                    .join("discover_stata_windows.bat"),
            );
        }
    }
    candidates.push(PathBuf::from("scripts").join("discover_stata_windows.bat"));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "discover_stata_windows.bat was not found; use the aiskill setup fallback".to_string()
        })
}

#[cfg(target_os = "windows")]
fn run_windows_discovery() -> Result<Value> {
    let script = discovery_script_path()?;
    let mut child = Command::new("cmd.exe")
        .args(["/d", "/s", "/c", "call"])
        .arg(&script)
        .args(["--stdout-only", "--no-pause"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to run {}: {error}", script.display()))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < Duration::from_secs(5) => {
                thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Windows Stata discovery exceeded 5000 ms".to_string());
            }
            Err(error) => return Err(format!("failed to wait for Windows discovery: {error}")),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("failed to collect Windows discovery output: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Windows Stata discovery failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim_start_matches('\u{feff}').trim())
        .map_err(|error| format!("invalid Windows discovery JSON: {error}"))
}

#[cfg(any(target_os = "windows", test))]
fn parse_windows_discovery_report(report: &Value) -> Result<Vec<DiscoveryCandidate>> {
    if report.get("schemaVersion").and_then(Value::as_u64) != Some(1) {
        return Err("unsupported Windows discovery schema".to_string());
    }
    let mut candidates = Vec::new();
    for value in report
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let executable = value
            .get("executablePath")
            .and_then(Value::as_str)
            .unwrap_or("");
        let library = value.get("dllPath").and_then(Value::as_str).unwrap_or("");
        if executable.is_empty() || library.is_empty() {
            continue;
        }
        let edition = value
            .get("dllEdition")
            .or_else(|| value.get("edition"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .map(|value| value as u32);
        let license_path = value
            .get("licensePath")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        candidates.push(DiscoveryCandidate {
            display_name: candidate_display_name(Path::new(executable), version, &edition),
            selected_path: PathBuf::from(executable),
            library_path: PathBuf::from(library),
            license_found: value
                .get("hasLicense")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    license_path
                        .as_ref()
                        .map(|path| path.exists())
                        .unwrap_or(false)
                }),
            license_path,
            edition,
            version,
            source: "windows_registry".to_string(),
        });
    }
    sort_candidates(&mut candidates);
    deduplicate_candidates(&mut candidates);
    Ok(candidates)
}

#[cfg(target_os = "windows")]
fn windows_libraries_in(dir: &Path) -> Vec<PathBuf> {
    [
        "mp-64.dll",
        "se-64.dll",
        "be-64.dll",
        "ic-64.dll",
        "StataMP-64.dll",
        "StataSE-64.dll",
        "StataBE-64.dll",
        "StataIC-64.dll",
    ]
    .iter()
    .map(|name| dir.join(name))
    .filter(|p| p.exists())
    .collect()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn scan_installations() -> (Vec<DiscoveryCandidate>, Option<String>) {
    (
        Vec::new(),
        Some("automatic Stata discovery is not supported on this platform".to_string()),
    )
}

fn example_paths() -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        return vec![
            "/Applications/StataMP.app".to_string(),
            "/Applications/StataNow/StataMP.app".to_string(),
            "/Applications/StataMP.app/Contents/MacOS/libstata-mp.dylib".to_string(),
        ];
    }
    #[cfg(target_os = "windows")]
    {
        return vec![
            r"C:\Program Files\Stata18".to_string(),
            r"C:\Program Files\Stata18\StataMP-64.exe".to_string(),
            r"C:\Program Files\Stata18\mp-64.dll".to_string(),
        ];
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    Vec::new()
}

struct RuntimeState {
    config: AppConfig,
    discovery: Discovery,
    session: Option<Arc<StataSession>>,
    init_error: Option<String>,
}

#[derive(Clone)]
struct SetupToken {
    value: String,
    created_at: Instant,
}

impl SetupToken {
    fn valid(&self, value: &str) -> bool {
        self.value == value && self.created_at.elapsed() <= SETUP_TOKEN_TTL
    }
}

struct SetupControl {
    phase: Option<String>,
    install_token: Option<SetupToken>,
    setup_tokens: Vec<SetupToken>,
    last_result: Option<String>,
}

struct AppState {
    paths: AppPaths,
    runtime: Mutex<RuntimeState>,
    setup: Mutex<SetupControl>,
    busy: AtomicBool,
    shutting_down: AtomicBool,
}

fn initialize_runtime(config: AppConfig) -> RuntimeState {
    let discovery = discover_stata(config.stata_path.as_deref());
    let (session, init_error) = initialize_session(&discovery);
    RuntimeState {
        config,
        discovery,
        session,
        init_error,
    }
}

fn initialize_session(discovery: &Discovery) -> (Option<Arc<StataSession>>, Option<String>) {
    if discovery.needs_license {
        let license_path = discovery
            .license_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|| "the expected Stata installation directory".to_string());
        return (None, Some(format!("Stata license file not found at {license_path}. Please make sure Stata is installed and licensed.")));
    }
    let Some(library) = &discovery.library_path else {
        return (None, None);
    };
    match StataSession::new(library) {
        Ok(session) => {
            let session = Arc::new(session);
            let _ = session.execute("quietly _gr_list on", false);
            (Some(session), None)
        }
        Err(error) => (None, Some(error)),
    }
}

fn serve(config: AppConfig, paths: AppPaths) -> Result<()> {
    let port = config.port;
    let state = Arc::new(AppState {
        paths,
        runtime: Mutex::new(initialize_runtime(config)),
        setup: Mutex::new(SetupControl {
            phase: None,
            install_token: None,
            setup_tokens: Vec::new(),
            last_result: None,
        }),
        busy: AtomicBool::new(false),
        shutting_down: AtomicBool::new(false),
    });
    let addr = format!("127.0.0.1:{port}");
    let listener =
        TcpListener::bind(&addr).map_err(|err| format!("failed to bind {addr}: {err}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("failed to set nonblocking listener: {err}"))?;
    eprintln!("Stata AI Skill listening on http://{addr}");

    while !state.shutting_down.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                let state = Arc::clone(&state);
                thread::spawn(move || {
                    let _ = handle_connection(stream, state);
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(format!("accept failed: {err}")),
        }
    }
    if let Ok(runtime) = state.runtime.lock() {
        if let Some(session) = &runtime.session {
            session.shutdown();
        }
    }
    Ok(())
}

fn handle_connection(mut stream: TcpStream, state: Arc<AppState>) -> Result<()> {
    let request = match read_http_request(&mut stream) {
        Ok(request) => request,
        Err(err) => return write_response(&mut stream, 400, &json_error(&err)),
    };
    let (head, body) = split_http_request(&request).unwrap_or((&request, ""));
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let uri = parts.next().unwrap_or("");
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect::<std::collections::HashMap<_, _>>();

    let (status, response, content_type) = match (method, path) {
        ("GET", "/status") if query_value(query, "format").as_deref() == Some("stata") => {
            (200, status_stata_text(&state), "text/plain; charset=utf-8")
        }
        ("GET", "/status") => (200, status_json(&state), "application/json"),
        ("POST", "/execute") => { let (status, body) = execute_json(&state, body); (status, body, "application/json") },
        ("POST", "/configure") => { let (status, body) = configure_json(&state, body); (status, body, "application/json") },
        ("POST", "/configure/reset") => {
            if let Err(error) = validate_setup_headers(&headers, &state) {
                (403, json_error(&error), "application/json")
            } else {
                let (status, body) = reset_configuration_json(&state);
                (status, body, "application/json")
            }
        },
        ("POST", "/setup/install-session") => {
            if let Err(error) = validate_setup_headers(&headers, &state) {
                (403, json_error(&error), "application/json")
            } else {
                let (status, body) = install_session_json(&state);
                (status, body, "application/json")
            }
        },
        ("GET", "/installed") => {
            if let Err(error) = validate_setup_headers(&headers, &state) {
                (403, stata_error_text(&error), "text/plain; charset=utf-8")
            } else {
                let (status, text) = installed_text(&state, query);
                (status, text, "text/plain; charset=utf-8")
            }
        }
        ("GET", "/setup") => {
            if let Err(error) = validate_setup_headers(&headers, &state) {
                (403, stata_error_text(&error), "text/plain; charset=utf-8")
            } else {
                let (status, text) = setup_text(Arc::clone(&state), query);
                (status, text, "text/plain; charset=utf-8")
            }
        }
        ("POST", "/break") => { let (status, body) = break_json(&state); (status, body, "application/json") },
        ("POST", "/shutdown") => { let (status, body) = shutdown_json(&state); (status, body, "application/json") },
        ("OPTIONS", _) => (204, String::new(), "application/json"),
        _ => (
            404,
            r#"{"success":false,"error":"Not Found. Available endpoints: GET /status, POST /configure, POST /configure/reset, POST /setup/install-session, GET /installed, GET /setup, POST /execute, POST /break, POST /shutdown"}"#.to_string(),
            "application/json",
        ),
    };
    write_response_with_type(&mut stream, status, &response, content_type)
}

fn read_http_request(stream: &mut TcpStream) -> Result<String> {
    let mut buffer = vec![0_u8; 8192];
    let mut request = Vec::new();
    let mut header_end = None;

    loop {
        let n = stream
            .read(&mut buffer)
            .map_err(|err| format!("failed to read request: {err}"))?;
        if n == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..n]);
        if request.len() > MAX_REQUEST_BYTES {
            return Err(format!("request exceeds {MAX_REQUEST_BYTES} bytes"));
        }
        if header_end.is_none() {
            header_end = find_header_end(&request);
        }
        if let Some(end) = header_end {
            let content_length = content_length_from_head(&request[..end])?;
            let total = end + 4 + content_length;
            if request.len() >= total {
                request.truncate(total);
                break;
            }
        }
    }

    String::from_utf8(request).map_err(|err| format!("request is not valid UTF-8: {err}"))
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

fn split_http_request(request: &str) -> Option<(&str, &str)> {
    request.split_once("\r\n\r\n")
}

fn content_length_from_head(head: &[u8]) -> Result<usize> {
    let head = String::from_utf8_lossy(head);
    for line in head.lines().skip(1) {
        if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                let length = value
                    .trim()
                    .parse::<usize>()
                    .map_err(|err| format!("invalid Content-Length: {err}"))?;
                if length > MAX_REQUEST_BYTES {
                    return Err(format!("request body exceeds {MAX_REQUEST_BYTES} bytes"));
                }
                return Ok(length);
            }
        }
    }
    Ok(0)
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> Result<()> {
    write_response_with_type(stream, status, body, "application/json")
}

fn write_response_with_type(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
    content_type: &str,
) -> Result<()> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        408 => "Request Timeout",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        426 => "Upgrade Required",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nCache-Control: no-store\r\nPragma: no-cache\r\nX-Content-Type-Options: nosniff\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|err| format!("failed to write response: {err}"))
}

fn status_json(state: &AppState) -> String {
    let runtime = match state.runtime.lock() {
        Ok(runtime) => runtime,
        Err(_) => return json_error("runtime state is unavailable"),
    };
    let session_active = runtime
        .session
        .as_ref()
        .map(|s| s.is_active())
        .unwrap_or(false);
    let needs_configuration = runtime.discovery.needs_configuration;
    let needs_license = runtime.discovery.needs_license;
    let message = if session_active {
        if state.busy.load(Ordering::SeqCst) {
            "Stata is busy executing".to_string()
        } else {
            "Stata session is active".to_string()
        }
    } else if needs_license {
        runtime.discovery.message.clone()
    } else if let Some(err) = &runtime.init_error {
        format!("Stata initialization failed: {err}. Use /configure with a confirmed candidate or guide the user through aiskill setup.")
    } else {
        runtime.discovery.message.clone()
    };
    let mut setup = state.setup.lock().ok();
    if let Some(setup) = setup.as_mut() {
        setup
            .setup_tokens
            .retain(|token| token.created_at.elapsed() <= SETUP_TOKEN_TTL);
        if setup
            .install_token
            .as_ref()
            .map(|token| token.created_at.elapsed() > SETUP_TOKEN_TTL)
            .unwrap_or(false)
        {
            setup.install_token = None;
            setup.phase = Some("manual_setup_required".to_string());
            setup.last_result = Some("Installation session expired".to_string());
        }
    }
    let phase = setup
        .as_ref()
        .and_then(|value| value.phase.clone())
        .unwrap_or_else(|| {
            if session_active {
                "ready".to_string()
            } else if runtime.init_error.is_some() || needs_license {
                "configuration_failed".to_string()
            } else if !runtime.discovery.candidates.is_empty() {
                "selection_required".to_string()
            } else {
                "manual_setup_required".to_string()
            }
        });
    let next_action = match phase.as_str() {
        "selection_required" => "ask_user_to_select_candidate",
        "manual_setup_required" => "start_aiskill_install_session",
        "awaiting_install_result" => "wait_for_installation_result",
        "awaiting_aiskill_setup" => "ask_user_to_run_aiskill_setup",
        "configuring" => "wait_for_configuration",
        "install_failed" => "offer_retry_or_skip",
        "configuration_failed" => "report_error_or_retry_setup",
        _ => "execute_stata",
    };
    let candidate_values = runtime
        .discovery
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| candidate.value(index == 0))
        .collect::<Vec<_>>();
    json!({
        "service": SERVICE_NAME,
        "skillVersion": SKILL_VERSION,
        "status": if state.shutting_down.load(Ordering::SeqCst) { "shutting_down" } else { "running" },
        "sessionActive": session_active,
        "busy": state.busy.load(Ordering::SeqCst),
        "needsConfiguration": needs_configuration,
        "needsLicense": needs_license,
        "licenseFound": runtime.discovery.license_found,
        "licensePath": runtime.discovery.license_path.as_ref().map(|p| p.to_string_lossy().to_string()),
        "missing": if needs_configuration && !runtime.discovery.candidates.is_empty() {
            Some("stata_selection")
        } else if needs_configuration {
            Some("stata_library_path")
        } else if needs_license {
            Some("stata_license")
        } else if runtime.init_error.is_some() {
            Some("stata_initialization")
        } else {
            None
        },
        "message": message,
        "examplePaths": &runtime.discovery.examples,
        "detectedCandidates": candidate_values,
        "recommendedCandidate": runtime.discovery.candidates.first().map(|candidate| candidate.value(true)),
        "discoveryError": &runtime.discovery.error,
        "initError": &runtime.init_error,
        "setup": {
            "phase": phase,
            "lastResult": setup.as_ref().and_then(|value| value.last_result.clone()),
            "nextAction": next_action,
            "sessionExpiresSeconds": SETUP_TOKEN_TTL.as_secs()
        },
        "config": {
            "port": runtime.config.port,
            "stataPath": runtime.config.stata_path.as_ref().map(|p| p.to_string_lossy().to_string()),
            "configFile": state.paths.config_file.to_string_lossy().to_string(),
            "logDir": state.paths.log_dir.to_string_lossy().to_string(),
            "tempDir": state.paths.temp_dir.to_string_lossy().to_string(),
            "graphDir": state.paths.graph_dir.to_string_lossy().to_string()
        },
        "capabilities": {
            "execute": true,
            "file": true,
            "cwd": true,
            "timeoutMaxSeconds": MAX_TIMEOUT_SEC,
            "graphs": "svg,png,jpg"
            ,"configure": true,
            "aiskillSetup": true
        }
    })
    .to_string()
}

fn random_token() -> Result<String> {
    let mut bytes = [0_u8; 24];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("failed to generate setup token: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn clean_stata_field(value: impl ToString) -> String {
    value.to_string().replace(['\r', '\n'], " ")
}

fn stata_text(fields: &[(&str, String)]) -> String {
    let mut output = format!("AISKILL/{SETUP_PROTOCOL_VERSION}\n");
    for (key, value) in fields {
        output.push_str(key);
        output.push('=');
        output.push_str(&clean_stata_field(value));
        output.push('\n');
    }
    output
}

fn stata_error_text(message: &str) -> String {
    stata_text(&[
        ("success", "0".to_string()),
        ("message", message.to_string()),
    ])
}

fn base_setup_phase(runtime: &RuntimeState) -> &'static str {
    let active = runtime
        .session
        .as_ref()
        .map(|session| session.is_active())
        .unwrap_or(false);
    if active {
        "ready"
    } else if runtime.init_error.is_some() || runtime.discovery.needs_license {
        "configuration_failed"
    } else if !runtime.discovery.candidates.is_empty() {
        "selection_required"
    } else {
        "manual_setup_required"
    }
}

fn status_stata_text(state: &AppState) -> String {
    let token = random_token().unwrap_or_else(|_| unique_id());
    if let Ok(mut setup) = state.setup.lock() {
        setup
            .setup_tokens
            .retain(|item| item.created_at.elapsed() <= SETUP_TOKEN_TTL);
        setup.setup_tokens.push(SetupToken {
            value: token.clone(),
            created_at: Instant::now(),
        });
        if setup.setup_tokens.len() > 8 {
            let remove_count = setup.setup_tokens.len() - 8;
            setup.setup_tokens.drain(..remove_count);
        }
    }
    let runtime = match state.runtime.lock() {
        Ok(runtime) => runtime,
        Err(_) => return stata_error_text("Runtime state is unavailable."),
    };
    let setup = state.setup.lock().ok();
    let phase = setup
        .as_ref()
        .and_then(|value| value.phase.as_deref())
        .unwrap_or_else(|| base_setup_phase(&runtime));
    let installation_path = runtime
        .config
        .stata_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default();
    let edition = runtime
        .discovery
        .library_path
        .as_ref()
        .map(|path| edition_from_library(path))
        .unwrap_or_default();
    let session_active = runtime
        .session
        .as_ref()
        .map(|session| session.is_active())
        .unwrap_or(false);
    stata_text(&[
        ("service", SERVICE_NAME.to_string()),
        ("protocolVersion", SETUP_PROTOCOL_VERSION.to_string()),
        ("skillVersion", SKILL_VERSION.to_string()),
        ("setupToken", token),
        (
            "configured",
            if runtime.config.stata_path.is_some() {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        ("installationPath", installation_path),
        ("stataEdition", edition.to_ascii_uppercase()),
        (
            "sessionActive",
            if session_active { "1" } else { "0" }.to_string(),
        ),
        ("setupPhase", phase.to_string()),
    ])
}

fn configure_json(state: &AppState, body: &str) -> (u16, String) {
    if state.busy.load(Ordering::SeqCst) {
        return (
            409,
            json_error("Stata is busy; configuration cannot change during execution"),
        );
    }
    let value: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(error) => return (400, json_error(&format!("invalid JSON: {error}"))),
    };
    let path = match value
        .get("stataPath")
        .and_then(Value::as_str)
        .map(str::trim)
    {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => return (400, json_error("`stataPath` must be a non-empty string")),
    };
    match configure_path(state, path) {
        Ok(()) => (200, status_json(state)),
        Err(error) => {
            if let Ok(mut setup) = state.setup.lock() {
                setup.phase = Some("configuration_failed".to_string());
                setup.last_result = Some(error.clone());
            }
            (422, json!({ "success": false, "error": error, "status": serde_json::from_str::<Value>(&status_json(state)).unwrap_or(Value::Null) }).to_string())
        }
    }
}

fn reset_configuration_json(state: &AppState) -> (u16, String) {
    if state.busy.load(Ordering::SeqCst) {
        return (
            409,
            json_error("Stata is busy; wait for execution to finish before reconfiguring"),
        );
    }
    if let Err(error) = remove_persisted_config(&state.paths) {
        return (500, json_error(&error));
    }
    state.shutting_down.store(true, Ordering::SeqCst);
    (
        200,
        json!({
            "success": true,
            "reset": true,
            "restartRequired": true,
            "nextAction": "restart_service_and_read_status",
            "message": "Persistent configuration cleared. Restart the service to begin setup again."
        })
        .to_string(),
    )
}

fn configure_path(state: &AppState, path: PathBuf) -> Result<()> {
    let (port, has_active_session, current_path) = {
        let runtime = state
            .runtime
            .lock()
            .map_err(|_| "runtime state is unavailable".to_string())?;
        (
            runtime.config.port,
            runtime
                .session
                .as_ref()
                .map(|session| session.is_active())
                .unwrap_or(false),
            runtime.config.stata_path.clone(),
        )
    };
    if has_active_session {
        if current_path.as_deref() == Some(path.as_path()) {
            if let Ok(mut setup) = state.setup.lock() {
                setup.phase = Some("ready".to_string());
                setup.last_result = Some(format!(
                    "Stata is already configured from {}",
                    path.display()
                ));
            }
            return Ok(());
        }
        return Err("A Stata session is already active. Shut down the service before selecting another installation.".to_string());
    }
    let config = AppConfig {
        port,
        stata_path: Some(path.clone()),
    };
    let next = initialize_runtime(config.clone());
    if next.discovery.library_path.is_none() {
        return Err(format!(
            "No compatible Stata library was found from {}",
            path.display()
        ));
    }
    if next.discovery.needs_license {
        return Err(next.discovery.message.clone());
    }
    if let Some(error) = &next.init_error {
        return Err(format!("Stata initialization failed: {error}"));
    }
    if next.session.as_ref().map(|session| session.is_active()) != Some(true) {
        return Err("Stata session did not become active".to_string());
    }
    config.save(&state.paths)?;
    let mut runtime = state
        .runtime
        .lock()
        .map_err(|_| "runtime state is unavailable".to_string())?;
    *runtime = next;
    drop(runtime);
    if let Ok(mut setup) = state.setup.lock() {
        setup.phase = Some("ready".to_string());
        setup.last_result = Some(format!("Configured Stata from {}", path.display()));
    }
    Ok(())
}

fn skill_root_from_executable() -> Option<PathBuf> {
    env::current_exe()
        .ok()?
        .parent()?
        .parent()?
        .parent()
        .map(Path::to_path_buf)
}

fn aiskill_package_dir() -> Result<PathBuf> {
    let candidates = [
        skill_root_from_executable().map(|root| root.join("stata").join("aiskill")),
        Some(PathBuf::from("stata").join("aiskill")),
        Some(
            PathBuf::from("skills")
                .join("stata-ai-skill")
                .join("stata")
                .join("aiskill"),
        ),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|path| path.join("aiskill.pkg").is_file())
        .ok_or_else(|| "The bundled aiskill Stata package was not found.".to_string())
}

fn install_session_json(state: &AppState) -> (u16, String) {
    let token = match random_token() {
        Ok(token) => token,
        Err(error) => return (500, json_error(&error)),
    };
    let package = match aiskill_package_dir() {
        Ok(path) => path,
        Err(error) => return (500, json_error(&error)),
    };
    let port = match state.runtime.lock() {
        Ok(runtime) => runtime.config.port,
        Err(_) => return (500, json_error("runtime state is unavailable")),
    };
    let installation_path = env::temp_dir().join("installation.do");
    let package_path = package
        .to_string_lossy()
        .replace('\\', "/")
        .replace('"', "\"\"");
    let script = format!(
        "tempfile __aiskill_install_response\ncapture noisily net install aiskill, from(\"{package_path}\") replace\nlocal __aiskill_install_rc = _rc\nif `__aiskill_install_rc' == 0 {{\n    capture quietly copy \"http://127.0.0.1:{port}/installed?aiskill=1&token={token}\" \"`__aiskill_install_response'\", replace\n}}\nelse {{\n    capture quietly copy \"http://127.0.0.1:{port}/installed?aiskill=0&token={token}\" \"`__aiskill_install_response'\", replace\n}}\n"
    );
    if let Err(error) = fs::write(&installation_path, script) {
        return (
            500,
            json_error(&format!("failed to create installation.do: {error}")),
        );
    }
    if let Ok(mut setup) = state.setup.lock() {
        setup.phase = Some("awaiting_install_result".to_string());
        setup.install_token = Some(SetupToken {
            value: token,
            created_at: Instant::now(),
        });
        setup.last_result = None;
    }
    (
        200,
        json!({
            "success": true,
            "phase": "awaiting_install_result",
            "command": "do \"`c(tmpdir)'/installation.do\"",
            "installationDo": installation_path.to_string_lossy(),
            "expiresSeconds": SETUP_TOKEN_TTL.as_secs()
        })
        .to_string(),
    )
}

fn installed_text(state: &AppState, query: &str) -> (u16, String) {
    let result = query_value(query, "aiskill");
    let token = query_value(query, "token").unwrap_or_default();
    if !matches!(result.as_deref(), Some("0") | Some("1")) {
        return (
            400,
            stata_error_text("The aiskill installation result must be 0 or 1."),
        );
    }
    let mut setup = match state.setup.lock() {
        Ok(setup) => setup,
        Err(_) => return (500, stata_error_text("Setup state is unavailable.")),
    };
    if !setup
        .install_token
        .as_ref()
        .map(|item| item.valid(&token))
        .unwrap_or(false)
    {
        return (
            403,
            stata_error_text("The installation token is invalid or expired."),
        );
    }
    setup.install_token = None;
    let installed = result.as_deref() == Some("1");
    setup.phase = Some(
        if installed {
            "awaiting_aiskill_setup"
        } else {
            "install_failed"
        }
        .to_string(),
    );
    setup.last_result = Some(
        if installed {
            "aiskill installed successfully"
        } else {
            "aiskill installation failed"
        }
        .to_string(),
    );
    (
        200,
        stata_text(&[
            ("success", "1".to_string()),
            ("installed", if installed { "1" } else { "0" }.to_string()),
            (
                "message",
                if installed {
                    "Installation acknowledged. Run aiskill setup."
                } else {
                    "Installation failure acknowledged."
                }
                .to_string(),
            ),
        ]),
    )
}

fn normalize_signal_platform(os: &str, machine_type: &str) -> String {
    let os = os.to_ascii_lowercase();
    let machine = machine_type.to_ascii_lowercase();
    if os.contains("windows") {
        "windows".to_string()
    } else if os.contains("mac") || (os == "unix" && machine.contains("mac")) {
        "macos".to_string()
    } else {
        os
    }
}

fn host_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "unsupported"
    }
}

fn resolve_signal_path(sysdir_values: &[String], flavor: &str) -> Option<PathBuf> {
    let requested = flavor.to_ascii_lowercase();
    let mut candidates = sysdir_values
        .iter()
        .flat_map(|value| resolve_from_user_path(Path::new(value)))
        .collect::<Vec<_>>();
    sort_candidates(&mut candidates);
    candidates
        .iter()
        .find(|candidate| candidate.edition == requested)
        .or_else(|| candidates.first())
        .map(|candidate| candidate.selected_path.clone())
}

fn setup_text(state: Arc<AppState>, query: &str) -> (u16, String) {
    if state.busy.load(Ordering::SeqCst) {
        return (409, stata_error_text("Run aiskill setup from a separately opened GUI Stata, not from the Stata AI Skill session."));
    }
    let required = [
        "protocolVersion",
        "setupToken",
        "clientVersion",
        "os",
        "stataVersion",
        "flavor",
        "machineType",
        "sysdirStata",
    ];
    let mut fields = std::collections::HashMap::new();
    for key in required {
        let Some(value) = query_value(query, key) else {
            return (
                400,
                stata_error_text(&format!("Missing setup field: {key}.")),
            );
        };
        fields.insert(key, value);
    }
    if fields.get("protocolVersion").map(String::as_str) != Some("1") {
        return (
            426,
            stata_error_text("The aiskill protocol version is incompatible."),
        );
    }
    if normalize_signal_platform(&fields["os"], &fields["machineType"]) != host_platform() {
        return (
            409,
            stata_error_text(
                "The running Stata platform does not match the Stata AI Skill service.",
            ),
        );
    }
    {
        let mut setup = match state.setup.lock() {
            Ok(setup) => setup,
            Err(_) => return (500, stata_error_text("Setup state is unavailable.")),
        };
        let token = &fields["setupToken"];
        let Some(index) = setup.setup_tokens.iter().position(|item| item.valid(token)) else {
            return (
                403,
                stata_error_text("The setup token is invalid or expired."),
            );
        };
        setup.setup_tokens.remove(index);
        setup.phase = Some("configuring".to_string());
        setup.last_result = Some("Configuration signal received from Stata".to_string());
    }
    let sysdir_values = query_value_candidates(query, "sysdirStata");
    let Some(path) = resolve_signal_path(&sysdir_values, &fields["flavor"]) else {
        if let Ok(mut setup) = state.setup.lock() {
            setup.phase = Some("configuration_failed".to_string());
            setup.last_result =
                Some("Could not resolve the Stata installation from c(sysdir_stata).".to_string());
        }
        return (
            422,
            stata_error_text("Could not resolve the Stata installation from c(sysdir_stata)."),
        );
    };
    thread::spawn(move || {
        if let Err(error) = configure_path(&state, path) {
            if let Ok(mut setup) = state.setup.lock() {
                setup.phase = Some("configuration_failed".to_string());
                setup.last_result = Some(error);
            }
        }
    });
    (
        200,
        stata_text(&[
            ("success", "1".to_string()),
            ("accepted", "1".to_string()),
            (
                "message",
                "Configuration received. Wait for the agent to confirm the result.".to_string(),
            ),
        ]),
    )
}

fn validate_setup_headers(
    headers: &std::collections::HashMap<String, String>,
    state: &AppState,
) -> Result<()> {
    if headers.contains_key("origin") {
        return Err("Browser-originated setup requests are not allowed.".to_string());
    }
    let port = state
        .runtime
        .lock()
        .map_err(|_| "runtime state is unavailable".to_string())?
        .config
        .port;
    let host = headers
        .get("host")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return Err("Invalid setup service host.".to_string());
    }
    Ok(())
}

fn percent_decode_bytes(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                output.push(byte);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            output.push(b' ');
        } else {
            output.push(bytes[index]);
        }
        index += 1;
    }
    output
}

fn query_value_candidates(query: &str, key: &str) -> Vec<String> {
    let Some(raw) = query.split('&').find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if name == key {
            Some(value)
        } else {
            None
        }
    }) else {
        return Vec::new();
    };
    let bytes = percent_decode_bytes(raw);
    let mut values = Vec::new();
    if let Ok(value) = String::from_utf8(bytes.clone()) {
        values.push(value);
    }
    for encoding in [GBK, BIG5, SHIFT_JIS, EUC_KR, WINDOWS_1252] {
        let (value, _, had_errors) = encoding.decode(&bytes);
        if !had_errors && !values.iter().any(|item| item == value.as_ref()) {
            values.push(value.into_owned());
        }
    }
    values
}

fn query_value(query: &str, key: &str) -> Option<String> {
    query_value_candidates(query, key).into_iter().next()
}

fn execute_json(state: &AppState, body: &str) -> (u16, String) {
    if state.shutting_down.load(Ordering::SeqCst) {
        return (503, json_error("Service is shutting down"));
    }
    let session = match state.runtime.lock() {
        Ok(runtime) => match &runtime.session {
            Some(session) if session.is_active() => Arc::clone(session),
            _ => {
                return (
                    503,
                    json_error(
                        "Stata session is not initialized. Check /status for setup.nextAction.",
                    ),
                )
            }
        },
        Err(_) => return (503, json_error("runtime state is unavailable")),
    };
    if state
        .busy
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return (409, json_error("Stata is busy executing another command"));
    }

    let request = match ExecuteRequest::parse(body) {
        Ok(request) => request,
        Err(err) => {
            state.busy.store(false, Ordering::SeqCst);
            return (400, json_error(&err));
        }
    };
    let result = execute_inner(state, session, request);
    state.busy.store(false, Ordering::SeqCst);
    result
}

fn execute_inner(
    state: &AppState,
    session: Arc<StataSession>,
    request: ExecuteRequest,
) -> (u16, String) {
    if request.code.trim().is_empty() && request.file.trim().is_empty() {
        return (400, json_error("Provide `code` or `file` in JSON body."));
    }
    let guard_text = if !request.file.trim().is_empty() {
        fs::read_to_string(&request.file).unwrap_or_default()
    } else {
        request.code.clone()
    };
    if contains_aiskill_self_call(&guard_text) {
        return (
            400,
            json_error(
                "Run `aiskill setup` or `aiskill status` from a separately opened GUI Stata, not through /execute.",
            ),
        );
    }
    let prepared = match prepare_command_with_cwd(state, &request.code, &request.file, &request.cwd)
    {
        Ok(prepared) => prepared,
        Err(err) => return (400, json_error(&err)),
    };
    let timeout = request
        .timeout
        .unwrap_or(DEFAULT_TIMEOUT_SEC)
        .min(MAX_TIMEOUT_SEC);
    let (tx, rx) = mpsc::channel();
    let run_session = Arc::clone(&session);
    let command = prepared.command.clone();
    thread::spawn(move || {
        let _ = tx.send(run_session.execute(&command, request.echo));
    });
    let mut result = match rx.recv_timeout(Duration::from_secs(timeout)) {
        Ok(result) => result,
        Err(_) => {
            session.set_break();
            match rx.recv_timeout(Duration::from_secs(10)) {
                Ok(mut result) => {
                    result.success = false;
                    result.return_code = -1;
                    result.error = format!("Execution timed out after {timeout}s");
                    result
                }
                Err(_) => ExecuteResult {
                    success: false,
                    return_code: -1,
                    output: format!("Execution timed out after {timeout}s"),
                    error: format!("Execution timed out after {timeout}s"),
                },
            }
        }
    };
    if let Some(path) = prepared.temp_file {
        let _ = fs::remove_file(path);
    }
    let (graphs, graph_notes) = if result.success {
        if prepared.graph_exports.is_empty() {
            (export_graphs(state, &session), Vec::new())
        } else {
            export_requested_graphs(state, &session, &prepared.graph_exports)
        }
    } else {
        (Vec::new(), Vec::new())
    };
    if result.success && !graph_notes.is_empty() {
        if !result.output.is_empty() {
            result.output.push_str("\n\n");
        }
        result
            .output
            .push_str(&format!("[Stata AI Skill] {}", graph_notes.join(" ")));
    }
    let status = if result.success {
        200
    } else if result.error.contains("timed out") {
        408
    } else {
        500
    };
    if !result.success && result.error.is_empty() {
        result.error = "Stata execution failed".to_string();
    }
    (
        status,
        format!(
            "{}",
            json!({
                "success": result.success,
                "returnCode": result.return_code,
                "output": result.output,
                "error": result.error,
                "graphs": graph_values(&graphs)
            })
        ),
    )
}

fn contains_aiskill_self_call(code: &str) -> bool {
    code.lines().any(|line| {
        let mut line = line.trim_start();
        if line.starts_with('*') || line.starts_with("//") {
            return false;
        }
        while let Some(rest) = [
            "quietly", "quietly:", "capture", "capture:", "noisily", "noisily:",
        ]
        .iter()
        .find_map(|prefix| {
            line.strip_prefix(prefix).and_then(|rest| {
                if rest.starts_with(char::is_whitespace) {
                    Some(rest.trim_start())
                } else {
                    None
                }
            })
        }) {
            line = rest;
        }
        let lower = line.to_ascii_lowercase();
        lower == "aiskill"
            || lower.starts_with("aiskill setup")
            || lower.starts_with("aiskill status")
    })
}

fn break_json(state: &AppState) -> (u16, String) {
    let stopped = state
        .runtime
        .lock()
        .ok()
        .and_then(|runtime| runtime.session.as_ref().map(|session| session.set_break()))
        .unwrap_or(false);
    (
        200,
        format!(
            "{{\"success\":{},\"message\":\"{}\"}}",
            stopped,
            if stopped {
                "Break signal sent"
            } else {
                "No active Stata session"
            }
        ),
    )
}

fn shutdown_json(state: &AppState) -> (u16, String) {
    state.shutting_down.store(true, Ordering::SeqCst);
    if let Ok(runtime) = state.runtime.lock() {
        if let Some(session) = &runtime.session {
            if state.busy.load(Ordering::SeqCst) {
                session.set_break();
            }
        }
    }
    (
        200,
        "{\"success\":true,\"message\":\"Service shutting down\"}".to_string(),
    )
}

#[derive(Debug, Default)]
struct ExecuteRequest {
    code: String,
    file: String,
    timeout: Option<u64>,
    echo: bool,
    cwd: String,
}

impl ExecuteRequest {
    fn parse(body: &str) -> Result<Self> {
        let value: Value =
            serde_json::from_str(body).map_err(|err| format!("invalid JSON body: {err}"))?;
        Ok(Self {
            code: optional_string(&value, "code")?.unwrap_or_default(),
            file: optional_string(&value, "file")?.unwrap_or_default(),
            timeout: optional_u64(&value, "timeout")?,
            echo: optional_bool(&value, "echo")?.unwrap_or(false),
            cwd: optional_string(&value, "cwd")?.unwrap_or_default(),
        })
    }
}

struct PreparedCommand {
    command: String,
    temp_file: Option<PathBuf>,
    graph_exports: Vec<GraphExportRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GraphExportRequest {
    original_line: String,
    target: PathBuf,
    bitmap_path: Option<PathBuf>,
    bitmap_format: Option<BitmapFormat>,
    name: Option<String>,
    replace: bool,
    requested_format: String,
    rewritten_to_svg: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BitmapFormat {
    Png,
    Jpg,
}

impl BitmapFormat {
    fn from_graph_format(format: &str) -> Option<Self> {
        match format.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpg" | "jpeg" => Some(Self::Jpg),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpg => "jpg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpg => "JPG",
        }
    }
}

fn prepare_command_with_cwd(
    state: &AppState,
    code: &str,
    file: &str,
    cwd: &str,
) -> Result<PreparedCommand> {
    let mut prefix = Vec::new();
    if !cwd.trim().is_empty() {
        prefix.push(format!("cd \"{}\"", escape_stata_path(cwd.trim())));
    }
    if !file.trim().is_empty() {
        prefix.push(format!("do \"{}\"", escape_stata_path(file.trim())));
        if prefix.len() > 1 {
            let path = write_temp_do_file(state, &prefix.join("\n"))?;
            return Ok(PreparedCommand {
                command: format!("do \"{}\"", escape_stata_path(&path.to_string_lossy())),
                temp_file: Some(path),
                graph_exports: Vec::new(),
            });
        }
        return Ok(PreparedCommand {
            command: prefix.join("\n"),
            temp_file: None,
            graph_exports: Vec::new(),
        });
    }
    let extracted = extract_graph_exports(code, cwd)?;
    let normalized = normalize_code(&extracted.code);
    if normalized.is_empty() {
        if prefix.is_empty() {
            prefix.push("display \"\"".to_string());
        }
        return Ok(PreparedCommand {
            command: prefix.join("\n"),
            temp_file: None,
            graph_exports: extracted.exports,
        });
    }
    let normalized = if prefix.is_empty() {
        normalized
    } else {
        prefix.push(normalized);
        prefix.join("\n")
    };
    if normalized
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count()
        > 1
    {
        let path = write_temp_do_file(state, &normalized)?;
        Ok(PreparedCommand {
            command: format!("do \"{}\"", escape_stata_path(&path.to_string_lossy())),
            temp_file: Some(path),
            graph_exports: extracted.exports,
        })
    } else {
        Ok(PreparedCommand {
            command: normalized,
            temp_file: None,
            graph_exports: extracted.exports,
        })
    }
}

fn write_temp_do_file(state: &AppState, code: &str) -> Result<PathBuf> {
    fs::create_dir_all(&state.paths.temp_dir)
        .map_err(|err| format!("failed to create temp dir: {err}"))?;
    let path = state
        .paths
        .temp_dir
        .join(format!("stata_ai_skill_{}.do", unique_id()));
    fs::write(&path, code).map_err(|err| format!("failed to write temp do file: {err}"))?;
    Ok(path)
}

fn normalize_code(code: &str) -> String {
    code.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix(". ")
                .or_else(|| trimmed.strip_prefix('.'))
                .unwrap_or(line)
                .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

struct ExtractedGraphExports {
    code: String,
    exports: Vec<GraphExportRequest>,
}

fn extract_graph_exports(code: &str, cwd: &str) -> Result<ExtractedGraphExports> {
    let mut kept = Vec::new();
    let mut exports = Vec::new();
    for line in code.replace("\r\n", "\n").replace('\r', "\n").lines() {
        if let Some(command) = graph_export_command(line) {
            exports.push(parse_graph_export(command, line.trim(), cwd)?);
        } else {
            kept.push(line.to_string());
        }
    }
    Ok(ExtractedGraphExports {
        code: kept.join("\n"),
        exports,
    })
}

fn graph_export_command(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    if let Some(after_dot) = rest.strip_prefix('.') {
        rest = after_dot.trim_start();
    }
    if starts_with_ascii_ci(rest, "quietly") {
        let after = &rest["quietly".len()..];
        if after
            .chars()
            .next()
            .map(char::is_whitespace)
            .unwrap_or(false)
        {
            rest = after.trim_start();
        }
    }
    if !starts_with_ascii_ci(rest, "graph") {
        return None;
    }
    let after_graph = &rest["graph".len()..];
    if !after_graph
        .chars()
        .next()
        .map(char::is_whitespace)
        .unwrap_or(false)
    {
        return None;
    }
    let rest = after_graph.trim_start();
    if !starts_with_ascii_ci(rest, "export") {
        return None;
    }
    let after_export = &rest["export".len()..];
    if !after_export.is_empty()
        && !after_export
            .chars()
            .next()
            .map(char::is_whitespace)
            .unwrap_or(false)
    {
        return None;
    }
    Some(after_export.trim_start())
}

fn parse_graph_export(command: &str, original_line: &str, cwd: &str) -> Result<GraphExportRequest> {
    let (path_text, options) = parse_graph_export_path(command)
        .ok_or_else(|| format!("failed to parse graph export path: {original_line}"))?;
    let name = parse_name_option(options);
    let replace = has_option_word(options, "replace");
    let option_format = parse_as_option(options);
    let requested_format = graph_format(&path_text, option_format.as_deref());
    let rewritten_to_svg = requested_format.to_ascii_lowercase() != "svg";
    let bitmap_format = BitmapFormat::from_graph_format(&requested_format);
    let bitmap_path = bitmap_format.map(|format| bitmap_output_path(&path_text, format, cwd));
    let target_text = if let Some(bitmap_path) = &bitmap_path {
        with_svg_extension(&bitmap_path.to_string_lossy())
    } else if rewritten_to_svg {
        with_svg_extension(&path_text)
    } else {
        ensure_svg_extension(&path_text)
    };
    let target = resolve_export_path(&target_text, cwd);
    Ok(GraphExportRequest {
        original_line: original_line.to_string(),
        target,
        bitmap_path,
        bitmap_format,
        name,
        replace,
        requested_format,
        rewritten_to_svg,
    })
}

fn parse_graph_export_path(command: &str) -> Option<(String, &str)> {
    let trimmed = command.trim_start();
    let quote = trimmed
        .chars()
        .next()
        .filter(|ch| *ch == '"' || *ch == '\'');
    if let Some(quote) = quote {
        let mut escaped = false;
        let mut end = None;
        for (idx, ch) in trimmed.char_indices().skip(1) {
            if ch == quote {
                if escaped {
                    escaped = false;
                    continue;
                }
                end = Some(idx);
                break;
            }
            escaped = ch == '\\' && !escaped;
        }
        let end = end?;
        let path = trimmed[1..end].to_string();
        let rest = trimmed[end + quote.len_utf8()..].trim_start();
        let options = rest.strip_prefix(',').unwrap_or("").trim_start();
        Some((path, options))
    } else {
        let split = trimmed
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace() || *ch == ',')
            .map(|(idx, _)| idx)
            .unwrap_or(trimmed.len());
        if split == 0 {
            return None;
        }
        let path = trimmed[..split].to_string();
        let rest = trimmed[split..].trim_start();
        let options = rest.strip_prefix(',').unwrap_or("").trim_start();
        Some((path, options))
    }
}

fn parse_name_option(options: &str) -> Option<String> {
    parse_parenthesized_option(options, "name")
}

fn parse_as_option(options: &str) -> Option<String> {
    parse_parenthesized_option(options, "as").map(|value| value.to_ascii_lowercase())
}

fn parse_parenthesized_option(options: &str, option: &str) -> Option<String> {
    let lower = options.to_ascii_lowercase();
    let needle = format!("{option}(");
    let start = lower.find(&needle)? + needle.len();
    let end = options[start..].find(')')? + start;
    let value = options[start..end].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn has_option_word(options: &str, option: &str) -> bool {
    options
        .split(|ch: char| ch.is_whitespace() || ch == ',')
        .any(|part| part.eq_ignore_ascii_case(option))
}

fn graph_format(path: &str, option_format: Option<&str>) -> String {
    option_format
        .map(ToString::to_string)
        .or_else(|| {
            Path::new(path)
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
        })
        .unwrap_or_else(|| "svg".to_string())
}

fn with_svg_extension(path: &str) -> String {
    let path = Path::new(path);
    let mut out = path.to_path_buf();
    out.set_extension("svg");
    out.to_string_lossy().to_string()
}

fn ensure_svg_extension(path: &str) -> String {
    if Path::new(path).extension().is_some() {
        path.to_string()
    } else {
        with_svg_extension(path)
    }
}

fn resolve_export_path(path: &str, cwd: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else if !cwd.trim().is_empty() {
        PathBuf::from(cwd.trim()).join(path)
    } else {
        env::current_dir()
            .unwrap_or_else(|_| env::temp_dir())
            .join(path)
    }
}

fn bitmap_output_path(path: &str, format: BitmapFormat, cwd: &str) -> PathBuf {
    let mut out = resolve_export_path(path, cwd);
    let ext = out
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let ext_matches = match (format, ext.as_deref()) {
        (BitmapFormat::Png, Some("png")) => true,
        (BitmapFormat::Jpg, Some("jpg" | "jpeg")) => true,
        _ => false,
    };
    if !ext_matches {
        out.set_extension(format.extension());
    }
    out
}

fn starts_with_ascii_ci(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .map(|part| part.eq_ignore_ascii_case(prefix))
        .unwrap_or(false)
}

#[derive(Clone)]
struct GraphExport {
    name: String,
    svg: PathBuf,
    png: Option<PathBuf>,
    file: Option<PathBuf>,
    format: Option<String>,
}

fn export_graphs(state: &AppState, session: &Arc<StataSession>) -> Vec<GraphExport> {
    let _ = session.execute("quietly _gr_list list", false);
    let result = session.execute("display \"`r(_grlist)'\"", false);
    let names = parse_graph_names(&result.output);
    let _ = fs::create_dir_all(&state.paths.graph_dir);
    let mut out = Vec::new();
    for (idx, name) in names.iter().enumerate() {
        let svg = state.paths.graph_dir.join(format!(
            "{}_{}_{}.svg",
            sanitize_filename(name),
            unique_id(),
            idx
        ));
        let cmd = format!(
            "quietly graph export \"{}\", name({}) replace",
            escape_stata_path(&svg.to_string_lossy()),
            name
        );
        let result = session.execute(&cmd, false);
        if result.success && svg.exists() {
            out.push(GraphExport {
                name: name.clone(),
                svg,
                png: None,
                file: None,
                format: None,
            });
        }
    }
    let _ = session.execute("quietly _gr_list clear", false);
    out
}

fn export_requested_graphs(
    _state: &AppState,
    session: &Arc<StataSession>,
    requests: &[GraphExportRequest],
) -> (Vec<GraphExport>, Vec<String>) {
    let names = current_graph_names(session);
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for request in requests {
        if let Some(parent) = request.target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let graph_name = request.name.clone().or_else(|| names.first().cloned());
        let mut options = Vec::new();
        if let Some(name) = &graph_name {
            options.push(format!("name({name})"));
        }
        if request.replace {
            options.push("replace".to_string());
        }
        let cmd = if options.is_empty() {
            format!(
                "quietly graph export \"{}\"",
                escape_stata_path(&request.target.to_string_lossy())
            )
        } else {
            format!(
                "quietly graph export \"{}\", {}",
                escape_stata_path(&request.target.to_string_lossy()),
                options.join(" ")
            )
        };
        let result = session.execute(&cmd, false);
        if result.success && request.target.exists() {
            let mut png = None;
            let mut file = None;
            let mut format = None;
            if let (Some(bitmap_path), Some(bitmap_format)) =
                (&request.bitmap_path, request.bitmap_format)
            {
                match convert_svg_to_bitmap(&request.target, bitmap_path, bitmap_format) {
                    Ok(()) => {
                        notes.push(format!(
                            "graph export requested {label}; exported SVG and converted bitmap: {path}",
                            label = bitmap_format.label(),
                            path = bitmap_path.to_string_lossy()
                        ));
                        file = Some(bitmap_path.clone());
                        format = Some(bitmap_format.extension().to_string());
                        if bitmap_format == BitmapFormat::Png {
                            png = Some(bitmap_path.clone());
                        }
                    }
                    Err(err) => {
                        notes.push(format!(
                            "graph export requested {label}, but SVG-to-{label} conversion failed: {err}. SVG kept: {path}",
                            label = bitmap_format.label(),
                            path = request.target.to_string_lossy()
                        ));
                    }
                }
            } else if request.rewritten_to_svg {
                notes.push(format!(
                    "graph export requested {format} output, exported SVG instead: {path}",
                    format = request.requested_format,
                    path = request.target.to_string_lossy()
                ));
            }
            out.push(GraphExport {
                name: graph_name.unwrap_or_else(|| "Graph".to_string()),
                svg: request.target.clone(),
                png,
                file,
                format,
            });
        }
    }
    let _ = session.execute("quietly _gr_list clear", false);
    (out, notes)
}

fn current_graph_names(session: &Arc<StataSession>) -> Vec<String> {
    let _ = session.execute("quietly _gr_list list", false);
    let result = session.execute("display \"`r(_grlist)'\"", false);
    parse_graph_names(&result.output)
}

fn convert_svg_to_bitmap(svg: &Path, dest: &Path, format: BitmapFormat) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create bitmap output directory: {err}"))?;
    }
    let mut options = resvg::usvg::Options {
        resources_dir: fs::canonicalize(svg)
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
        ..resvg::usvg::Options::default()
    };
    options.fontdb_mut().load_system_fonts();
    let svg_data = fs::read(svg).map_err(|err| format!("failed to read SVG: {err}"))?;
    let tree = resvg::usvg::Tree::from_data(&svg_data, &options)
        .map_err(|err| format!("failed to parse SVG: {err}"))?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| "failed to create bitmap buffer".to_string())?;
    resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());
    let width = pixmap.width();
    let height = pixmap.height();
    let rgba = pixmap.take_demultiplied();
    match format {
        BitmapFormat::Png => image::save_buffer_with_format(
            dest,
            &rgba,
            width,
            height,
            image::ColorType::Rgba8,
            image::ImageFormat::Png,
        )
        .map_err(|err| format!("failed to write PNG: {err}")),
        BitmapFormat::Jpg => {
            let rgb = rgba_to_rgb_on_white(&rgba);
            image::save_buffer_with_format(
                dest,
                &rgb,
                width,
                height,
                image::ColorType::Rgb8,
                image::ImageFormat::Jpeg,
            )
            .map_err(|err| format!("failed to write JPG: {err}"))
        }
    }
}

fn rgba_to_rgb_on_white(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for pixel in rgba.chunks_exact(4) {
        let alpha = pixel[3] as u16;
        for channel in &pixel[..3] {
            let channel = *channel as u16;
            let composited = (channel * alpha + 255 * (255 - alpha) + 127) / 255;
            rgb.push(composited as u8);
        }
    }
    rgb
}

fn parse_graph_names(output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for item in output.split_whitespace() {
        let mut chars = item.chars();
        let valid_start = chars
            .next()
            .map(|ch| ch.is_ascii_alphabetic() || ch == '_')
            .unwrap_or(false);
        if valid_start
            && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            && !out.iter().any(|existing| existing == item)
        {
            out.push(item.to_string());
        }
    }
    out
}

fn sanitize_filename(value: &str) -> String {
    let out: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "graph".to_string()
    } else {
        out
    }
}

#[derive(Debug)]
struct ExecuteResult {
    success: bool,
    return_code: i32,
    output: String,
    error: String,
}

struct StataSession {
    platform: PlatformSession,
    active: AtomicBool,
}

impl StataSession {
    fn new(library_path: &Path) -> Result<Self> {
        Ok(Self {
            platform: PlatformSession::new(library_path)?,
            active: AtomicBool::new(true),
        })
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    fn execute(&self, code: &str, echo: bool) -> ExecuteResult {
        self.platform.execute(code, echo)
    }

    fn set_break(&self) -> bool {
        self.platform.set_break()
    }

    fn shutdown(&self) {
        if self.active.swap(false, Ordering::SeqCst) {
            self.platform.shutdown();
        }
    }
}

struct NativeApi {
    handle: LibraryHandle,
    main: StataMain,
    execute: StataExecute,
    clear_output: StataClearOutput,
    get_output: StataGetOutput,
    set_break: StataSetBreak,
    shutdown: StataShutdown,
}

unsafe impl Send for NativeApi {}

type StataMain = unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int;
type StataExecute = unsafe extern "C" fn(*const c_char, c_int) -> c_int;
type StataClearOutput = unsafe extern "C" fn();
type StataGetOutput = unsafe extern "C" fn() -> *mut c_char;
type StataSetBreak = unsafe extern "C" fn();
type StataShutdown = unsafe extern "C" fn();

impl NativeApi {
    unsafe fn load(path: &Path) -> Result<Self> {
        let handle = LibraryHandle::open(path)?;
        Ok(Self {
            main: handle.symbol("StataSO_Main")?,
            execute: handle.symbol("StataSO_Execute")?,
            clear_output: handle.symbol("StataSO_ClearOutputBuffer")?,
            get_output: handle.symbol("StataSO_GetOutputBuffer")?,
            set_break: handle.symbol("StataSO_SetBreak")?,
            shutdown: handle.symbol("StataSO_Shutdown")?,
            handle,
        })
    }

    fn init(&self, library_path: &Path) -> Result<()> {
        set_stata_environment(library_path);
        let mut args = init_args();
        let mut argv: Vec<*mut c_char> = args
            .iter_mut()
            .map(|arg| arg.as_ptr() as *mut c_char)
            .collect();
        let rc = unsafe { (self.main)(argv.len() as c_int, argv.as_mut_ptr()) };
        if rc >= 0 || rc == -7100 {
            Ok(())
        } else {
            Err(format!("StataSO_Main failed with return code {rc}"))
        }
    }

    fn execute(&self, code: &str, echo: bool) -> ExecuteResult {
        let code = match CString::new(code) {
            Ok(code) => code,
            Err(err) => {
                return ExecuteResult {
                    success: false,
                    return_code: -1,
                    output: String::new(),
                    error: format!("Stata code contains NUL byte: {err}"),
                }
            }
        };
        unsafe {
            (self.clear_output)();
            let rc = (self.execute)(code.as_ptr(), if echo { 1 } else { 0 });
            let output = self.output();
            ExecuteResult {
                success: rc == 0,
                return_code: rc,
                output,
                error: if rc == 0 {
                    String::new()
                } else {
                    format!("StataSO_Execute failed with return code {rc}")
                },
            }
        }
    }

    fn output(&self) -> String {
        unsafe {
            let ptr = (self.get_output)();
            if ptr.is_null() {
                String::new()
            } else {
                CStr::from_ptr(ptr).to_string_lossy().into_owned()
            }
        }
    }

    fn shutdown(&self) {
        unsafe { (self.shutdown)() }
    }
}

impl Drop for NativeApi {
    fn drop(&mut self) {
        let _ = &self.handle;
    }
}

#[cfg(not(target_os = "windows"))]
struct PlatformSession {
    api: Mutex<NativeApi>,
    break_fn: StataSetBreak,
}

#[cfg(not(target_os = "windows"))]
impl PlatformSession {
    fn new(library_path: &Path) -> Result<Self> {
        let api = unsafe { NativeApi::load(library_path)? };
        api.init(library_path)?;
        let break_fn = api.set_break;
        Ok(Self {
            api: Mutex::new(api),
            break_fn,
        })
    }

    fn execute(&self, code: &str, echo: bool) -> ExecuteResult {
        match self.api.lock() {
            Ok(api) => api.execute(code, echo),
            Err(_) => ExecuteResult {
                success: false,
                return_code: -1,
                output: String::new(),
                error: "Stata session mutex poisoned".to_string(),
            },
        }
    }

    fn set_break(&self) -> bool {
        unsafe { (self.break_fn)() };
        true
    }

    fn shutdown(&self) {
        if let Ok(api) = self.api.lock() {
            api.shutdown();
        }
    }
}

#[cfg(target_os = "windows")]
struct PlatformSession {
    tx: mpsc::Sender<WorkerCommand>,
    break_fn: Arc<Mutex<Option<StataSetBreak>>>,
}

#[cfg(target_os = "windows")]
enum WorkerCommand {
    Execute {
        code: String,
        echo: bool,
        reply: mpsc::Sender<ExecuteResult>,
    },
    Shutdown,
}

#[cfg(target_os = "windows")]
impl PlatformSession {
    fn new(library_path: &Path) -> Result<Self> {
        let library_path = library_path.to_path_buf();
        let (tx, rx) = mpsc::channel();
        let (init_tx, init_rx) = mpsc::channel();
        let break_fn = Arc::new(Mutex::new(None));
        let worker_break_fn = Arc::clone(&break_fn);
        thread::spawn(move || {
            let init = unsafe { NativeApi::load(&library_path) }.and_then(|api| {
                api.init(&library_path)?;
                if let Ok(mut slot) = worker_break_fn.lock() {
                    *slot = Some(api.set_break);
                }
                Ok(api)
            });
            let api = match init {
                Ok(api) => {
                    let _ = init_tx.send(Ok(()));
                    api
                }
                Err(err) => {
                    let _ = init_tx.send(Err(err));
                    return;
                }
            };
            while let Ok(command) = rx.recv() {
                match command {
                    WorkerCommand::Execute { code, echo, reply } => {
                        let _ = reply.send(api.execute(&code, echo));
                    }
                    WorkerCommand::Shutdown => {
                        api.shutdown();
                        break;
                    }
                }
            }
        });
        match init_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx, break_fn }),
            Ok(Err(err)) => Err(err),
            Err(err) => Err(format!("Stata worker failed to initialize: {err}")),
        }
    }

    fn execute(&self, code: &str, echo: bool) -> ExecuteResult {
        let (reply, rx) = mpsc::channel();
        if self
            .tx
            .send(WorkerCommand::Execute {
                code: code.to_string(),
                echo,
                reply,
            })
            .is_err()
        {
            return ExecuteResult {
                success: false,
                return_code: -1,
                output: String::new(),
                error: "Stata worker is not running".to_string(),
            };
        }
        rx.recv().unwrap_or_else(|err| ExecuteResult {
            success: false,
            return_code: -1,
            output: String::new(),
            error: format!("Stata worker failed: {err}"),
        })
    }

    fn set_break(&self) -> bool {
        self.break_fn
            .lock()
            .ok()
            .and_then(|slot| *slot)
            .map(|f| unsafe { f() })
            .is_some()
    }

    fn shutdown(&self) {
        let _ = self.tx.send(WorkerCommand::Shutdown);
    }
}

fn set_stata_environment(library_path: &Path) {
    if let Some(home) = derive_stata_home(library_path) {
        env::set_var("SYSDIR_STATA", &home);
        #[cfg(target_os = "windows")]
        {
            env::set_var("STATA", &home);
            let _ = env::set_current_dir(&home);
        }
    }
}

fn derive_stata_home(library_path: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        library_path.parent().map(Path::to_path_buf)
    }
    #[cfg(target_os = "macos")]
    {
        let macos_dir = library_path.parent()?;
        let contents_dir = macos_dir.parent()?;
        let app_dir = contents_dir.parent()?;
        app_dir.parent().map(Path::to_path_buf)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        library_path.parent().map(Path::to_path_buf)
    }
}

fn expected_license_path(library_path: &Path) -> Option<PathBuf> {
    let home = derive_stata_home(library_path)?;
    if let Ok(entries) = fs::read_dir(&home) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_license = path
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.eq_ignore_ascii_case("stata.lic"))
                .unwrap_or(false);
            if is_license {
                return Some(path);
            }
        }
    }
    Some(home.join("stata.lic"))
}

fn init_args() -> Vec<CString> {
    #[cfg(target_os = "windows")]
    {
        vec![CString::new("stata").unwrap(), CString::new("-q").unwrap()]
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![
            CString::new("").unwrap(),
            CString::new("-q").unwrap(),
            CString::new("-pyexec").unwrap(),
            CString::new("").unwrap(),
        ]
    }
}

#[cfg(not(target_os = "windows"))]
struct LibraryHandle(*mut c_void);

#[cfg(not(target_os = "windows"))]
impl LibraryHandle {
    unsafe fn open(path: &Path) -> Result<Self> {
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|err| format!("invalid library path: {err}"))?;
        let handle = dlopen(path.as_ptr(), RTLD_LAZY);
        if handle.is_null() {
            Err(format!(
                "failed to open Stata library: {}",
                dlerror_string()
            ))
        } else {
            Ok(Self(handle))
        }
    }

    unsafe fn symbol<T: Copy>(&self, name: &str) -> Result<T> {
        let name = CString::new(name).unwrap();
        let symbol = dlsym(self.0, name.as_ptr());
        if symbol.is_null() {
            Err(format!(
                "failed to resolve symbol {}: {}",
                name.to_string_lossy(),
                dlerror_string()
            ))
        } else {
            Ok(std::mem::transmute_copy(&symbol))
        }
    }
}

#[cfg(not(target_os = "windows"))]
impl Drop for LibraryHandle {
    fn drop(&mut self) {
        unsafe {
            dlclose(self.0);
        }
    }
}

#[cfg(not(target_os = "windows"))]
const RTLD_LAZY: c_int = 0x1;

#[cfg(not(target_os = "windows"))]
extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
    fn dlerror() -> *const c_char;
}

#[cfg(not(target_os = "windows"))]
fn dlerror_string() -> String {
    unsafe {
        let err = dlerror();
        if err.is_null() {
            "unknown dlopen error".to_string()
        } else {
            CStr::from_ptr(err).to_string_lossy().into_owned()
        }
    }
}

#[cfg(target_os = "windows")]
struct LibraryHandle(*mut c_void);

#[cfg(target_os = "windows")]
impl LibraryHandle {
    unsafe fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            let old = env::var("PATH").unwrap_or_default();
            env::set_var("PATH", format!("{};{}", parent.display(), old));
        }
        let path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|err| format!("invalid library path: {err}"))?;
        let handle = LoadLibraryA(path.as_ptr());
        if handle.is_null() {
            Err("failed to open Stata DLL".to_string())
        } else {
            Ok(Self(handle))
        }
    }

    unsafe fn symbol<T: Copy>(&self, name: &str) -> Result<T> {
        let name = CString::new(name).unwrap();
        let symbol = GetProcAddress(self.0, name.as_ptr());
        if symbol.is_null() {
            Err(format!(
                "failed to resolve symbol {}",
                name.to_string_lossy()
            ))
        } else {
            Ok(std::mem::transmute_copy(&symbol))
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for LibraryHandle {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}

#[cfg(target_os = "windows")]
extern "system" {
    fn LoadLibraryA(filename: *const c_char) -> *mut c_void;
    fn GetProcAddress(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn FreeLibrary(handle: *mut c_void) -> c_int;
}

fn json_error(message: &str) -> String {
    json!({
        "success": false,
        "output": "",
        "error": message
    })
    .to_string()
}

fn escape_stata_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\"\"")
}

fn unique_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let counter = UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}_{}_{}", millis, std::process::id(), counter)
}

fn graph_values(graphs: &[GraphExport]) -> Vec<Value> {
    graphs
        .iter()
        .map(|graph| {
            let mut value = json!({
                "name": graph.name,
                "svg": graph.svg.to_string_lossy().to_string(),
                "png": graph.png.as_ref().map(|path| path.to_string_lossy().to_string())
            });
            if let Some(file) = &graph.file {
                value["file"] = json!(file.to_string_lossy().to_string());
            }
            if let Some(format) = &graph.format {
                value["format"] = json!(format);
            }
            value
        })
        .collect()
}

fn optional_string(value: &Value, key: &str) -> Result<Option<String>> {
    match value.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("`{key}` must be a string")),
    }
}

fn optional_u64(value: &Value, key: &str) -> Result<Option<u64>> {
    match value.get(key) {
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("`{key}` must be a non-negative integer")),
    }
}

fn optional_bool(value: &Value, key: &str) -> Result<Option<bool>> {
    match value.get(key) {
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("`{key}` must be a boolean")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        let root = env::temp_dir().join(format!("stata-ai-skill-test-{}", unique_id()));
        let config = AppConfig {
            port: 19522,
            stata_path: None,
        };
        let discovery = Discovery {
            library_path: None,
            license_path: None,
            license_found: false,
            needs_configuration: true,
            needs_license: false,
            message: "test".to_string(),
            examples: Vec::new(),
            candidates: Vec::new(),
            error: None,
        };
        AppState {
            paths: AppPaths {
                config_file: root.join("config.toml"),
                log_dir: root.join("logs"),
                temp_dir: root.join("tmp"),
                graph_dir: root.join("graphs"),
                config_dir: root,
            },
            runtime: Mutex::new(RuntimeState {
                config,
                discovery,
                session: None,
                init_error: None,
            }),
            setup: Mutex::new(SetupControl {
                phase: None,
                install_token: None,
                setup_tokens: Vec::new(),
                last_result: None,
            }),
            busy: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
        }
    }

    #[test]
    fn parses_execute_request_with_unicode_escapes() {
        let request = ExecuteRequest::parse(
            r#"{"code":"display \"\u4ef7\u683c\"","timeout":45,"echo":true,"cwd":"/tmp/stata"}"#,
        )
        .unwrap();

        assert_eq!(request.code, "display \"价格\"");
        assert_eq!(request.timeout, Some(45));
        assert!(request.echo);
        assert_eq!(request.cwd, "/tmp/stata");
    }

    #[test]
    fn rejects_invalid_execute_field_types() {
        let err = ExecuteRequest::parse(r#"{"code":123}"#).unwrap_err();
        assert_eq!(err, "`code` must be a string");
    }

    #[test]
    fn extracts_content_length_case_insensitively() {
        let length =
            content_length_from_head(b"POST /execute HTTP/1.1\r\nhost: x\r\ncontent-length: 17")
                .unwrap();

        assert_eq!(length, 17);
    }

    #[test]
    fn prepares_cwd_before_inline_code() {
        let state = test_state();
        let command =
            prepare_command_with_cwd(&state, "display 2+2", "", "/Users/example project").unwrap();
        let temp_file = command.temp_file.as_ref().unwrap();
        let temp_code = fs::read_to_string(temp_file).unwrap();

        assert_eq!(temp_code, "cd \"/Users/example project\"\ndisplay 2+2");
        assert_eq!(
            command.command,
            format!("do \"{}\"", escape_stata_path(&temp_file.to_string_lossy()))
        );
        assert!(command.graph_exports.is_empty());
        fs::remove_file(temp_file).unwrap();
    }

    #[test]
    fn prepares_cwd_before_do_file() {
        let state = test_state();
        let command = prepare_command_with_cwd(&state, "", "analysis.do", "/tmp/work").unwrap();

        let temp_file = command.temp_file.as_ref().unwrap();
        let temp_code = fs::read_to_string(temp_file).unwrap();
        assert_eq!(temp_code, "cd \"/tmp/work\"\ndo \"analysis.do\"");
        assert_eq!(
            command.command,
            format!("do \"{}\"", escape_stata_path(&temp_file.to_string_lossy()))
        );
        assert!(command.graph_exports.is_empty());
        fs::remove_file(temp_file).unwrap();
    }

    #[test]
    fn recognizes_dot_and_quietly_graph_export_lines() {
        assert!(graph_export_command(". graph export \"a.svg\", replace").is_some());
        assert!(graph_export_command("   quietly graph export \"a.svg\", replace").is_some());

        let extracted = extract_graph_exports(
            "sysuse auto, clear\n. graph export \"a.svg\", replace\n quietly graph export \"b.svg\", replace",
            "",
        )
        .unwrap();

        assert_eq!(extracted.code, "sysuse auto, clear");
        assert_eq!(extracted.exports.len(), 2);
        assert_eq!(extracted.exports[0].target.file_name().unwrap(), "a.svg");
        assert_eq!(extracted.exports[1].target.file_name().unwrap(), "b.svg");
    }

    #[test]
    fn parses_user_export_path_and_name_option() {
        let request = parse_graph_export(
            "\"figures/result.svg\", replace name(my_graph)",
            "graph export \"figures/result.svg\", replace name(my_graph)",
            "/tmp/project",
        )
        .unwrap();

        assert_eq!(
            request.target,
            PathBuf::from("/tmp/project/figures/result.svg")
        );
        assert_eq!(request.name.as_deref(), Some("my_graph"));
        assert!(request.replace);
        assert_eq!(request.requested_format, "svg");
        assert!(!request.rewritten_to_svg);
    }

    #[test]
    fn rewrites_bitmap_graph_exports_to_svg_path() {
        for path in ["plot.png", "plot.jpg", "plot.jpeg", "plot.tif", "plot.tiff"] {
            let request = parse_graph_export(
                &format!("\"{path}\", replace name(g1)"),
                "graph export bitmap",
                "/tmp/project",
            )
            .unwrap();

            assert_eq!(
                request.target,
                PathBuf::from("/tmp/project")
                    .join(path)
                    .with_extension("svg")
            );
            assert_eq!(request.name.as_deref(), Some("g1"));
            assert!(request.rewritten_to_svg);
            if path.ends_with(".png") {
                assert_eq!(request.bitmap_format, Some(BitmapFormat::Png));
                assert_eq!(
                    request.bitmap_path.as_ref().unwrap(),
                    &PathBuf::from("/tmp/project/plot.png")
                );
            } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
                assert_eq!(request.bitmap_format, Some(BitmapFormat::Jpg));
                assert_eq!(
                    request.bitmap_path.as_ref().unwrap(),
                    &PathBuf::from("/tmp/project").join(path)
                );
            } else {
                assert_eq!(request.bitmap_format, None);
                assert_eq!(request.bitmap_path, None);
            }
        }
    }

    #[test]
    fn keeps_auto_graph_export_when_no_explicit_export_exists() {
        let state = test_state();
        let command =
            prepare_command_with_cwd(&state, "sysuse auto, clear\nscatter price mpg", "", "")
                .unwrap();

        assert!(command.graph_exports.is_empty());
        let temp_file = command.temp_file.as_ref().unwrap();
        let temp_code = fs::read_to_string(temp_file).unwrap();
        assert_eq!(temp_code, "sysuse auto, clear\nscatter price mpg");
        fs::remove_file(temp_file).unwrap();
    }

    #[test]
    fn cwd_multiline_code_uses_temp_do_file_and_preserves_cd_prefix() {
        let state = test_state();
        let command = prepare_command_with_cwd(
            &state,
            "sysuse auto, clear\n. graph export \"out.png\", replace\nsummarize price",
            "",
            "/tmp/work",
        )
        .unwrap();

        let temp_file = command.temp_file.as_ref().unwrap();
        let temp_code = fs::read_to_string(temp_file).unwrap();
        assert_eq!(
            temp_code,
            "cd \"/tmp/work\"\nsysuse auto, clear\nsummarize price"
        );
        assert_eq!(
            command.command,
            format!("do \"{}\"", escape_stata_path(&temp_file.to_string_lossy()))
        );
        assert_eq!(command.graph_exports.len(), 1);
        assert_eq!(
            command.graph_exports[0].target,
            PathBuf::from("/tmp/work/out.svg")
        );
        assert_eq!(
            command.graph_exports[0].bitmap_path.as_ref().unwrap(),
            &PathBuf::from("/tmp/work/out.png")
        );
        fs::remove_file(temp_file).unwrap();
    }

    #[test]
    fn converts_svg_to_png_and_jpg() {
        let root = env::temp_dir().join(format!("stata-ai-skill-convert-{}", unique_id()));
        fs::create_dir_all(&root).unwrap();
        let svg = root.join("input.svg");
        let png = root.join("output.png");
        let jpg = root.join("output.jpg");
        fs::write(
            &svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="16"><rect width="24" height="16" fill="#3366cc"/></svg>"##,
        )
        .unwrap();

        convert_svg_to_bitmap(&svg, &png, BitmapFormat::Png).unwrap();
        convert_svg_to_bitmap(&svg, &jpg, BitmapFormat::Jpg).unwrap();

        let png_bytes = fs::read(&png).unwrap();
        let jpg_bytes = fs::read(&jpg).unwrap();
        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(jpg_bytes.starts_with(&[0xff, 0xd8, 0xff]));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_unique_graph_names() {
        let names = parse_graph_names("Graph g1 g1 _tmp invalid-name 2bad");

        assert_eq!(names, vec!["Graph", "g1", "_tmp"]);
    }

    #[test]
    fn sorts_discovery_candidates_by_version_then_edition() {
        let candidate = |version, edition: &str, name: &str| DiscoveryCandidate {
            display_name: name.to_string(),
            selected_path: PathBuf::from(name),
            library_path: PathBuf::from(format!("{name}.dll")),
            license_path: None,
            license_found: true,
            edition: edition.to_string(),
            version: Some(version),
            source: "test".to_string(),
        };
        let mut candidates = vec![
            candidate(18, "mp", "Stata18MP"),
            candidate(19, "se", "Stata19SE"),
            candidate(19, "mp", "Stata19MP"),
        ];
        sort_candidates(&mut candidates);
        assert_eq!(candidates[0].display_name, "Stata19MP");
        assert_eq!(candidates[1].display_name, "Stata19SE");
        assert_eq!(candidates[2].display_name, "Stata18MP");
    }

    #[test]
    fn parses_windows_bat_schema_and_candidates() {
        let report = json!({
            "schemaVersion": 1,
            "candidates": [{
                "executablePath": "C:\\Program Files\\Stata19\\StataMP-64.exe",
                "dllPath": "C:\\Program Files\\Stata19\\mp-64.dll",
                "licensePath": "C:\\Program Files\\Stata19\\stata.lic",
                "hasLicense": true,
                "edition": "mp",
                "version": 19
            }]
        });
        let candidates = parse_windows_discovery_report(&report).unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].edition, "mp");
        assert_eq!(candidates[0].version, Some(19));
        assert!(candidates[0].license_found);
        assert_eq!(
            parse_windows_discovery_report(&json!({ "schemaVersion": 2 })).unwrap_err(),
            "unsupported Windows discovery schema"
        );
    }

    #[test]
    fn json_status_does_not_rotate_stata_setup_tokens() {
        let state = test_state();
        let first = status_stata_text(&state);
        assert!(first.starts_with("AISKILL/1\n"));
        let before = state.setup.lock().unwrap().setup_tokens[0].value.clone();
        let json: Value = serde_json::from_str(&status_json(&state)).unwrap();
        assert_eq!(json["setup"]["phase"], "manual_setup_required");
        assert_eq!(json["setup"]["nextAction"], "start_aiskill_install_session");
        let after = state.setup.lock().unwrap().setup_tokens[0].value.clone();
        assert_eq!(before, after);
    }

    #[test]
    fn reset_configuration_deletes_persistence_and_requests_restart() {
        let state = test_state();
        let persisted = AppConfig {
            port: 19522,
            stata_path: Some(PathBuf::from("/previous/Stata.app")),
        };
        persisted.save(&state.paths).unwrap();
        state.runtime.lock().unwrap().config = persisted;
        {
            let mut setup = state.setup.lock().unwrap();
            setup.phase = Some("awaiting_aiskill_setup".to_string());
            setup.install_token = Some(SetupToken {
                value: "old-install-token".to_string(),
                created_at: Instant::now(),
            });
            setup.setup_tokens.push(SetupToken {
                value: "old-setup-token".to_string(),
                created_at: Instant::now(),
            });
        }

        let (status_code, body) = reset_configuration_json(&state);
        assert_eq!(status_code, 200);
        let response: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["success"], true);
        assert_eq!(response["reset"], true);
        assert_eq!(response["restartRequired"], true);
        assert_eq!(response["nextAction"], "restart_service_and_read_status");
        assert!(!state.paths.config_file.exists());
        assert!(state.shutting_down.load(Ordering::SeqCst));
    }

    #[test]
    fn reset_configuration_refuses_while_stata_is_busy() {
        let state = test_state();
        state.busy.store(true, Ordering::SeqCst);
        let (status_code, body) = reset_configuration_json(&state);
        assert_eq!(status_code, 409);
        assert!(body.contains("wait for execution to finish"));
    }

    #[test]
    fn config_reset_is_idempotent() {
        let state = test_state();
        AppConfig {
            port: 19522,
            stata_path: Some(PathBuf::from("/previous/Stata.app")),
        }
        .save(&state.paths)
        .unwrap();
        remove_persisted_config(&state.paths).unwrap();
        remove_persisted_config(&state.paths).unwrap();
        assert!(!state.paths.config_file.exists());
    }

    #[test]
    fn skill_maps_reconfigure_requests_to_persistent_reset() {
        let skill = fs::read_to_string("skills/stata-ai-skill/SKILL.md").unwrap();
        assert!(skill.contains("重新配置该技能"));
        assert!(skill.contains("POST http://127.0.0.1:19522/configure/reset"));
        assert!(skill.contains("stata-ai-skill config reset"));
        assert!(skill.contains("do not ask for another confirmation"));
        assert!(skill.contains("Do not use `aiskill setup, force` as a"));
    }

    #[test]
    fn install_session_waits_for_valid_single_use_callback() {
        let state = test_state();
        let (status, body) = install_session_json(&state);
        assert_eq!(status, 200);
        let response: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(response["phase"], "awaiting_install_result");
        assert_eq!(response["command"], "do \"`c(tmpdir)'/installation.do\"");
        let token = state
            .setup
            .lock()
            .unwrap()
            .install_token
            .as_ref()
            .unwrap()
            .value
            .clone();
        let (callback_status, callback) =
            installed_text(&state, &format!("aiskill=1&token={token}"));
        assert_eq!(callback_status, 200);
        assert!(callback.starts_with("AISKILL/1\n"));
        assert_eq!(
            state.setup.lock().unwrap().phase.as_deref(),
            Some("awaiting_aiskill_setup")
        );
        assert_eq!(
            installed_text(&state, &format!("aiskill=1&token={token}")).0,
            403
        );
        let _ = fs::remove_file(env::temp_dir().join("installation.do"));
    }

    #[test]
    fn decodes_utf8_and_gbk_setup_paths() {
        assert_eq!(
            query_value("sysdirStata=C%3A%5CStata+19", "sysdirStata").as_deref(),
            Some("C:\\Stata 19")
        );
        let values = query_value_candidates("sysdirStata=%D6%D0%CE%C4", "sysdirStata");
        assert!(values.iter().any(|value| value == "\u{4e2d}\u{6587}"));
    }

    #[test]
    fn bundled_aiskill_command_has_no_saio_or_auxiliary_port_scan() {
        let ado = fs::read_to_string("stata/aiskill/aiskill.ado").unwrap();
        assert!(!ado.to_ascii_lowercase().contains("saio"));
        assert!(!ado.contains("16886"));
        assert!(!ado.contains("16895"));
        assert!(!ado
            .to_ascii_lowercase()
            .contains("syntax [anything(name=command)] [, port"));
        assert!(ado.contains("127.0.0.1:19522"));
    }

    #[test]
    fn blocks_aiskill_self_setup_and_status_calls() {
        assert!(contains_aiskill_self_call("aiskill setup"));
        assert!(contains_aiskill_self_call("quietly: aiskill status"));
        assert!(contains_aiskill_self_call("capture noisily aiskill"));
        assert!(!contains_aiskill_self_call("aiskill version"));
        assert!(!contains_aiskill_self_call("* aiskill setup"));
    }
}
