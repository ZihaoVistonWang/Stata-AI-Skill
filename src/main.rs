use std::env;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const SERVICE_NAME: &str = "stata-all-in-one-ai-skill";
const SKILL_VERSION: &str = "202606130001";
const DEFAULT_PORT: u16 = 19522;
const DEFAULT_TIMEOUT_SEC: u64 = 30;
const MAX_TIMEOUT_SEC: u64 = 600;
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

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
        _ => {
            eprintln!(
                "Usage:\n  stata-ai-skill serve [--stata-path <path>] [--port <port>]\n  stata-ai-skill config set [--stata-path <path>] [--port <port>]\n  stata-ai-skill config show"
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

fn parse_quoted_value(line: &str) -> Option<String> {
    let start = line.find('"')? + 1;
    let end = line[start..].rfind('"')? + start;
    Some(line[start..end].replace("\\\"", "\"").replace("\\\\", "\\"))
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
    candidates: Vec<PathBuf>,
}

fn discover_stata(configured: Option<&Path>) -> Discovery {
    let mut candidates = Vec::new();
    if let Some(path) = configured {
        candidates.extend(resolve_from_user_path(path));
    }
    candidates.extend(scan_common_paths());
    let library_path = candidates.iter().find(|p| p.exists()).cloned();
    let license_path = library_path.as_deref().and_then(expected_license_path);
    let license_found = license_path.as_ref().map(|p| p.exists()).unwrap_or(false);
    let needs_configuration = library_path.is_none();
    let needs_license = library_path.is_some() && !license_found;
    Discovery {
        library_path,
        license_path,
        license_found,
        needs_configuration,
        needs_license,
        message: if needs_configuration {
            format!(
                "Stata was not found automatically. Ask the user where the Stata app/program is installed. Examples: {}. Then run `stata-ai-skill config set --stata-path <path>`.",
                example_paths().join(" or ")
            )
        } else if needs_license {
            "Stata was found, but the license file stata.lic / STATA.lic was not found in the expected Stata installation directory. Ask the user to confirm Stata is licensed and that the license file exists next to the Stata installation.".to_string()
        } else {
            "Stata library and license file found.".to_string()
        },
        examples: example_paths(),
        candidates,
    }
}

fn resolve_from_user_path(path: &Path) -> Vec<PathBuf> {
    if is_library_path(path) {
        return vec![path.to_path_buf()];
    }
    #[cfg(target_os = "macos")]
    {
        if path.extension().and_then(|v| v.to_str()) == Some("app") {
            return macos_libraries_in(&path.join("Contents").join("MacOS"));
        }
        if path.is_dir() {
            return macos_libraries_in(path);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if path.is_file() {
            if let Some(parent) = path.parent() {
                return windows_libraries_in(parent);
            }
        }
        if path.is_dir() {
            return windows_libraries_in(path);
        }
    }
    Vec::new()
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
fn scan_common_paths() -> Vec<PathBuf> {
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

    apps.iter()
        .flat_map(|app| resolve_from_user_path(app))
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
fn scan_common_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();

    let drives = ('A'..='Z')
        .map(|d| PathBuf::from(format!("{}:\\", d)))
        .filter(|p| {
            // is_dir is fast on Windows for non-existent drives;
            // only call when the path actually exists to avoid
            // unnecessary stat calls on removable / network drives.
            p.exists()
        })
        .collect::<Vec<_>>();

    for drive in &drives {
        // drive:\Program Files\Stata*  (catches C:\Program Files\Stata18, etc.)
        for prog_dir in ["Program Files", "Program Files (x86)"] {
            let pf = drive.join(prog_dir);
            if pf.is_dir() {
                if let Ok(entries) = fs::read_dir(&pf) {
                    for entry in entries.flatten() {
                        let name = entry
                            .file_name()
                            .to_str()
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if name.starts_with("stata") {
                            out.extend(windows_libraries_in(&entry.path()));
                        }
                    }
                }
            }
        }

        // drive root: drive:\Stata*  (catches D:\Stata18, etc.)
        if let Ok(entries) = fs::read_dir(drive) {
            for entry in entries.flatten() {
                let name = entry
                    .file_name()
                    .to_str()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                if name.starts_with("stata") && entry.path().is_dir() {
                    out.extend(windows_libraries_in(&entry.path()));
                }
            }
        }
    }
    out
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
fn scan_common_paths() -> Vec<PathBuf> {
    Vec::new()
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

struct AppState {
    config: AppConfig,
    paths: AppPaths,
    discovery: Discovery,
    session: Option<Arc<StataSession>>,
    init_error: Option<String>,
    busy: AtomicBool,
    shutting_down: AtomicBool,
}

fn serve(config: AppConfig, paths: AppPaths) -> Result<()> {
    let port = config.port;
    let discovery = discover_stata(config.stata_path.as_deref());
    let (session, init_error) = if discovery.needs_license {
        let license_path = discovery
            .license_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| "the expected Stata installation directory".to_string());
        (
            None,
            Some(format!(
                "Stata license file not found at {license_path}. Please make sure Stata is installed and licensed."
            )),
        )
    } else if let Some(lib) = &discovery.library_path {
        match StataSession::new(lib) {
            Ok(session) => (Some(Arc::new(session)), None),
            Err(err) => (None, Some(err)),
        }
    } else {
        (None, None)
    };

    let state = Arc::new(AppState {
        config,
        paths,
        discovery,
        session,
        init_error,
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
    if let Some(session) = &state.session {
        session.shutdown();
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
    let path = parts.next().unwrap_or("");

    let (status, json) = match (method, path) {
        ("GET", "/status") => (200, status_json(&state)),
        ("POST", "/execute") => execute_json(&state, body),
        ("POST", "/break") => break_json(&state),
        ("POST", "/shutdown") => shutdown_json(&state),
        ("OPTIONS", _) => (204, String::new()),
        _ => (
            404,
            r#"{"success":false,"error":"Not Found. Available endpoints: GET /status, POST /execute, POST /break, POST /shutdown"}"#.to_string(),
        ),
    };
    write_response(&mut stream, status, &json)
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
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        408 => "Request Timeout",
        409 => "Conflict",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|err| format!("failed to write response: {err}"))
}

fn status_json(state: &AppState) -> String {
    let session_active = state
        .session
        .as_ref()
        .map(|s| s.is_active())
        .unwrap_or(false);
    let needs_configuration = state.discovery.needs_configuration;
    let needs_license = state.discovery.needs_license;
    let message = if session_active {
        if state.busy.load(Ordering::SeqCst) {
            "Stata is busy executing".to_string()
        } else {
            "Stata session is active".to_string()
        }
    } else if needs_license {
        state.discovery.message.clone()
    } else if let Some(err) = &state.init_error {
        format!("Stata initialization failed: {err}. Ask the user where the Stata app/program is installed, then reconfigure with `stata-ai-skill config set --stata-path <path>`.")
    } else {
        state.discovery.message.clone()
    };
    json!({
        "service": SERVICE_NAME,
        "skillVersion": SKILL_VERSION,
        "status": if state.shutting_down.load(Ordering::SeqCst) { "shutting_down" } else { "running" },
        "sessionActive": session_active,
        "busy": state.busy.load(Ordering::SeqCst),
        "needsConfiguration": needs_configuration,
        "needsLicense": needs_license,
        "licenseFound": state.discovery.license_found,
        "licensePath": state.discovery.license_path.as_ref().map(|p| p.to_string_lossy().to_string()),
        "missing": if needs_configuration {
            Some("stata_library_path")
        } else if needs_license {
            Some("stata_license")
        } else if state.init_error.is_some() {
            Some("stata_initialization")
        } else {
            None
        },
        "message": message,
        "examplePaths": &state.discovery.examples,
        "detectedCandidates": paths_to_strings(&state.discovery.candidates),
        "initError": &state.init_error,
        "config": {
            "port": state.config.port,
            "stataPath": state.config.stata_path.as_ref().map(|p| p.to_string_lossy().to_string()),
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
            "graphs": "svg"
        }
    })
    .to_string()
}

fn execute_json(state: &AppState, body: &str) -> (u16, String) {
    if state.shutting_down.load(Ordering::SeqCst) {
        return (503, json_error("Service is shutting down"));
    }
    let session = match &state.session {
        Some(session) if session.is_active() => Arc::clone(session),
        _ => {
            return (
                503,
                json_error(
                    "Stata session is not initialized. Check /status for needsConfiguration.",
                ),
            )
        }
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
    let graphs = if result.success {
        export_graphs(state, &session)
    } else {
        Vec::new()
    };
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

fn break_json(state: &AppState) -> (u16, String) {
    let stopped = state
        .session
        .as_ref()
        .map(|s| s.set_break())
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
    if let Some(session) = &state.session {
        if state.busy.load(Ordering::SeqCst) {
            session.set_break();
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
        return Ok(PreparedCommand {
            command: prefix.join("\n"),
            temp_file: None,
        });
    }
    let normalized = normalize_code(&strip_graph_export(code));
    if normalized.is_empty() {
        prefix.push(
            "display \"graph export command ignored; graphs are exported automatically\""
                .to_string(),
        );
        return Ok(PreparedCommand {
            command: prefix.join("\n"),
            temp_file: None,
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
        fs::create_dir_all(&state.paths.temp_dir)
            .map_err(|err| format!("failed to create temp dir: {err}"))?;
        let path = state
            .paths
            .temp_dir
            .join(format!("stata_ai_skill_{}.do", unique_id()));
        fs::write(&path, normalized)
            .map_err(|err| format!("failed to write temp do file: {err}"))?;
        Ok(PreparedCommand {
            command: format!("do \"{}\"", escape_stata_path(&path.to_string_lossy())),
            temp_file: Some(path),
        })
    } else {
        Ok(PreparedCommand {
            command: normalized,
            temp_file: None,
        })
    }
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

fn strip_graph_export(code: &str) -> String {
    code.lines()
        .filter(|line| {
            let lower = line
                .trim_start()
                .trim_start_matches('.')
                .trim_start()
                .to_ascii_lowercase();
            !(lower.starts_with("graph export") || lower.starts_with("quietly graph export"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone)]
struct GraphExport {
    name: String,
    svg: PathBuf,
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
            });
        }
    }
    let _ = session.execute("quietly _gr_list clear", false);
    out
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
    format!("{}_{}", millis, std::process::id())
}

fn paths_to_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

fn graph_values(graphs: &[GraphExport]) -> Vec<Value> {
    graphs
        .iter()
        .map(|graph| {
            json!({
                "name": graph.name,
                "svg": graph.svg.to_string_lossy().to_string(),
                "png": Value::Null
            })
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
        AppState {
            config: AppConfig {
                port: 19522,
                stata_path: None,
            },
            paths: AppPaths {
                config_file: root.join("config.toml"),
                log_dir: root.join("logs"),
                temp_dir: root.join("tmp"),
                graph_dir: root.join("graphs"),
                config_dir: root,
            },
            discovery: Discovery {
                library_path: None,
                license_path: None,
                license_found: false,
                needs_configuration: true,
                needs_license: false,
                message: "test".to_string(),
                examples: Vec::new(),
                candidates: Vec::new(),
            },
            session: None,
            init_error: None,
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
        fs::remove_file(temp_file).unwrap();
    }

    #[test]
    fn prepares_cwd_before_do_file() {
        let state = test_state();
        let command = prepare_command_with_cwd(&state, "", "analysis.do", "/tmp/work").unwrap();

        assert_eq!(command.command, "cd \"/tmp/work\"\ndo \"analysis.do\"");
    }

    #[test]
    fn parses_unique_graph_names() {
        let names = parse_graph_names("Graph g1 g1 _tmp invalid-name 2bad");

        assert_eq!(names, vec!["Graph", "g1", "_tmp"]);
    }
}
