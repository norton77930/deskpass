//! `list_apps` (ticket 78): the installed apps, merged from four sources into one entry per app.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, ERROR_SUCCESS, PROPERTYKEY};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, WIN32_FIND_DATAW};
use windows::Win32::Storage::Packaging::Appx::{AppxFactory, GetApplicationUserModelId, GetPackagePathByFullName, GetPackagesByPackageFamily, IAppxFactory};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, IBindCtx, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ, STGM_SHARE_DENY_NONE};
use windows::Win32::System::Registry::{RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_BINARY};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Shell::{
    IEnumShellItems, IShellItem, IShellItem2, IShellLinkW, SHCreateItemFromParsingName, SHCreateStreamOnFileEx, SHGetKnownFolderItem, SHGetKnownFolderPath, BHID_EnumItems,
    FOLDERID_AppsFolder, FOLDERID_CommonPrograms, FOLDERID_Programs, FOLDERID_Windows, KF_FLAG_DEFAULT, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, ShellLink,
};

use crate::windows_list;

/// The most apps one answer lists; `total` still counts all of them.
const MAX_APPS: usize = 200;

/// Test-only overrides, so a test never touches the user's own Start Menu or UserAssist: a
/// `;`-separated list of folders that replaces both Start Menu Programs roots, and an HKCU subkey
/// path that replaces the UserAssist root.
const TEST_START_MENU_DIRS: &str = "CUMCP_TEST_START_MENU_DIRS";
const TEST_USER_ASSIST_KEY: &str = "CUMCP_TEST_USERASSIST_KEY";
const USER_ASSIST_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist";

/// The installed apps, one entry each, merged from `shell:AppsFolder`, the Start Menu shortcuts,
/// UserAssist (run count and last run) and the open windows' processes (running), newest used
/// first. Only the first two list apps; the other two add to the apps they found. Each source
/// that cannot be read adds nothing.
pub fn list_apps() -> Result<Value, String> {
    let mut catalog = listed_catalog();
    for (name, count, last_run) in user_assist(&std::env::var(TEST_USER_ASSIST_KEY).unwrap_or_else(|_| USER_ASSIST_KEY.to_string())) {
        if let Some(target) = classify(&rot13(&name), &known_folder_path) {
            catalog.used(&target, count, last_run);
        }
    }
    for target in running_targets() {
        catalog.running(&target);
    }
    let (apps, total) = catalog.finish(MAX_APPS);
    Ok(json!({ "apps": apps.iter().map(App::to_json).collect::<Vec<_>>(), "total": total }))
}

/// How many executables `list_start_menu_apps` works out the identity of at once: verifying a
/// signature against the system catalogs can take seconds for one file.
const IDENTITY_WORKERS: usize = 8;

/// `list_start_menu_apps` (ticket 116): every app the Start Menu lists (`shell:AppsFolder` and the
/// shortcuts, as `list_apps` finds them, uncut), for the settings page's "All apps" view: `{ apps:
/// [{ app_id, name, key, program? }] }`, by name. `key` is the identity key the server's approval
/// check gives the app (`identityKey`, src/identity.ts): a package by its family, a program (an
/// `exe:` app's path, the program a `shell:` app runs) by its identity as `app_identity` reports
/// it, an unsigned one by its path; null for an entry that names no program, and for one whose
/// program is not on a fixed local drive (UNC, mapped, removable: `on_fixed_drive`, ticket 83's
/// guard), which is never opened, so a dead share cannot hold the answer up. Such an app cannot be
/// raised ahead of time: a key guessed from its path alone could differ from the signer's key the
/// approval check would give it, and the raise would not apply. Nothing is filtered: which entries
/// are apps is the server's to decide.
pub fn list_start_menu_apps() -> Value {
    let (apps, _) = listed_catalog().finish(usize::MAX);
    let entries: Vec<StartMenuEntry> = apps.iter().map(StartMenuEntry::of).collect();
    let keys = program_keys(&programs_to_identify(&entries, &drive_type));
    start_menu_json(&entries, &|program| keys.get(&program.to_lowercase()).cloned())
}

/// Ticket 116: the programs of `entries` whose identity is looked up, each once (compared without
/// regard to case): only those on a fixed local drive (`on_fixed_drive`).
fn programs_to_identify(entries: &[StartMenuEntry], drive_type: &dyn Fn(&str) -> u32) -> Vec<String> {
    let mut programs: Vec<String> = entries.iter().filter_map(|entry| entry.program.clone()).filter(|program| on_fixed_drive(program, drive_type)).collect();
    programs.sort_by_key(|program| program.to_lowercase());
    programs.dedup_by_key(|program| program.to_lowercase());
    programs
}

/// Ticket 116: one Start Menu entry as `list_start_menu_apps` reports it, before its key.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StartMenuEntry {
    app_id: String,
    name: String,
    /// A packaged app's family, lowercase.
    family: Option<String>,
    /// The executable it runs, when known.
    program: Option<String>,
}

impl StartMenuEntry {
    fn of(app: &App) -> StartMenuEntry {
        let (family, program) = match &app.target {
            AppTarget::Uwp(aumid) => (Some(aumid.split_once('!').map(|(family, _)| family).unwrap_or(aumid).to_lowercase()), None),
            AppTarget::Exe(path) => (None, Some(path.clone())),
            AppTarget::Shell(_) => (None, app.program.clone()),
        };
        StartMenuEntry { app_id: app.target.app_id(), name: app.display_name(), family, program: program.filter(|program| !program.is_empty()) }
    }
}

/// Ticket 116: the entries as JSON, each program's key from `key_of` (None: no key).
fn start_menu_json(entries: &[StartMenuEntry], key_of: &dyn Fn(&str) -> Option<String>) -> Value {
    let apps: Vec<Value> = entries
        .iter()
        .map(|entry| {
            let key = match (&entry.family, &entry.program) {
                (Some(family), _) => Some(format!("package:{family}")),
                (None, Some(program)) => key_of(program),
                (None, None) => None,
            };
            let mut app = json!({ "app_id": entry.app_id, "name": entry.name, "key": key });
            if let Some(program) = &entry.program {
                app["program"] = json!(program);
            }
            app
        })
        .collect();
    json!({ "apps": apps })
}

/// Ticket 116: the identity key of each of `programs` (by its lowercase path), as the server's
/// approval check makes it (`identity::identity_key` of the `app_identity` answer), worked out by
/// `IDENTITY_WORKERS` threads.
fn program_keys(programs: &[String]) -> std::collections::HashMap<String, String> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let found = std::sync::Mutex::new(std::collections::HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..IDENTITY_WORKERS.min(programs.len()) {
            scope.spawn(|| {
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                }
                loop {
                    let index = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let Some(program) = programs.get(index) else { break };
                    let path = program.replace('/', "\\");
                    let identity = crate::identity::app_identity(&json!({ "path": path })).ok();
                    let key = crate::identity::identity_key(program, identity.as_ref());
                    if let Ok(mut found) = found.lock() {
                        found.insert(program.to_lowercase(), key);
                    }
                }
            });
        }
    });
    found.into_inner().unwrap_or_default()
}

/// The apps the two listing sources (`shell:AppsFolder` and the Start Menu shortcuts) name.
fn listed_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let explorer = windows_explorer().unwrap_or_default();
    for (name, parsing_name, item) in apps_folder_items() {
        let Some(target) = classify(&parsing_name, &known_folder_path).filter(is_app) else {
            continue;
        };
        // Ticket 83: the program a shell: entry runs, read from the item already in hand.
        let program = matches!(target, AppTarget::Shell(_)).then(|| item.cast::<IShellItem2>().ok().and_then(|item| link_program(&item))).flatten();
        if is_folder_link(&target, program.as_deref(), &explorer) {
            continue;
        }
        catalog.add(target.clone(), &name);
        if let Some(program) = program {
            catalog.program(&target, program);
        }
    }
    for dir in start_menu_dirs() {
        for (name, program) in shortcuts(&dir) {
            let target = AppTarget::Exe(program);
            if !is_folder_link(&target, None, &explorer) {
                catalog.add(target, &name);
            }
        }
    }
    catalog
}

/// `resolve_app_id` (ticket 79): the executable an `app_id` from `list_apps` runs, which the server
/// approves and checks against the high-risk list before anything starts: `{ app }`, and for a
/// `uwp:` id also `{ aumid }`, which `launch_app` starts it by. An `exe:` id is its own path; a
/// `shell:` id is the program its `shell:AppsFolder` item points to, started by that path. An id
/// `list_apps` never gives is `invalid_params`; one that names nothing installed, or no program,
/// `launch_failed`. Messages never quote the id.
pub fn resolve_app_id(params: &Value) -> Result<Value, (String, String)> {
    let app_id = params.get("app_id").and_then(Value::as_str).unwrap_or("");
    let target = AppTarget::from_app_id(app_id).ok_or_else(|| {
        (
            "invalid_params".to_string(),
            "app_id is not one computer_list_apps gives. Call computer_list_apps and pass an app_id from it exactly.".to_string(),
        )
    })?;
    match target {
        AppTarget::Uwp(aumid) => Ok(json!({ "app": uwp_executable(&aumid)?, "aumid": aumid })),
        AppTarget::Exe(path) if Path::new(&path).is_file() => Ok(json!({ "app": path })),
        AppTarget::Exe(_) => Err(launch_failed("That app's executable is no longer there. Call computer_list_apps again.")),
        AppTarget::Shell(name) => Ok(json!({ "app": shell_executable(&name)? })),
    }
}

fn launch_failed(message: &str) -> (String, String) {
    ("launch_failed".to_string(), message.to_string())
}

const NO_PROGRAM: &str = "Could not tell which executable that app runs, so it cannot be approved or launched.";

/// The executable a packaged app (`Family!App`) runs: the `Executable` its package manifest gives
/// that application, in the package's install folder, for the first package of the family
/// installed for this user that has one. It is what the app's windows report as their `app`
/// (ticket 34), so approving it covers operating them.
pub(crate) fn uwp_executable(aumid: &str) -> Result<String, (String, String)> {
    let family = aumid.split_once('!').map(|(family, _)| family).unwrap_or(aumid);
    let packages = family_packages(family);
    if packages.is_empty() {
        return Err(launch_failed("That app is not installed for this user. Call computer_list_apps again."));
    }
    for package in packages {
        let Some(dir) = package_path(&package) else {
            continue;
        };
        let Some(relative) = manifest_executable(&dir, aumid) else {
            continue;
        };
        let path = format!("{}\\{relative}", dir.trim_end_matches('\\'));
        if Path::new(&path).is_file() {
            return Ok(path);
        }
    }
    Err(launch_failed(NO_PROGRAM))
}

/// The full names of the packages of `family` installed for this user.
fn family_packages(family: &str) -> Vec<String> {
    let wide = windows_list::utf16(family);
    unsafe {
        let (mut count, mut length) = (0u32, 0u32);
        let sized = GetPackagesByPackageFamily(PCWSTR(wide.as_ptr()), &mut count, None, &mut length, None);
        if (sized != ERROR_SUCCESS && sized != ERROR_INSUFFICIENT_BUFFER) || count == 0 {
            return Vec::new();
        }
        let mut names = vec![PWSTR::null(); count as usize];
        let mut buffer = vec![0u16; length as usize];
        if GetPackagesByPackageFamily(PCWSTR(wide.as_ptr()), &mut count, Some(names.as_mut_ptr()), &mut length, Some(PWSTR(buffer.as_mut_ptr()))) != ERROR_SUCCESS {
            return Vec::new();
        }
        names.iter().take(count as usize).filter_map(|name| name.to_string().ok()).collect()
    }
}

/// The install folder of the package `full_name`.
pub(crate) fn package_path(full_name: &str) -> Option<String> {
    let wide = windows_list::utf16(full_name);
    unsafe {
        let mut length = 0u32;
        let sized = GetPackagePathByFullName(PCWSTR(wide.as_ptr()), &mut length, None);
        if sized != ERROR_INSUFFICIENT_BUFFER || length == 0 {
            return None;
        }
        let mut buffer = vec![0u16; length as usize];
        if GetPackagePathByFullName(PCWSTR(wide.as_ptr()), &mut length, Some(PWSTR(buffer.as_mut_ptr()))) != ERROR_SUCCESS {
            return None;
        }
        let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..end]))
    }
}

/// The `Executable` of the application `aumid` in the manifest in `dir`, relative to `dir`. None
/// when the application has none (a web app) or it leaves the folder.
fn manifest_executable(dir: &str, aumid: &str) -> Option<String> {
    unsafe {
        let manifest = windows_list::utf16(&format!("{}\\AppxManifest.xml", dir.trim_end_matches('\\')));
        let stream = SHCreateStreamOnFileEx(PCWSTR(manifest.as_ptr()), (STGM_READ | STGM_SHARE_DENY_NONE).0, 0, false, None).ok()?;
        let factory: IAppxFactory = CoCreateInstance(&AppxFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let reader = factory.CreateManifestReader(&stream).ok()?;
        let applications = reader.GetApplications().ok()?;
        while applications.GetHasCurrent().ok()?.as_bool() {
            let application = applications.GetCurrent().ok()?;
            let id = application.GetAppUserModelId().ok().and_then(|id| take_string(id));
            if id.is_some_and(|id| id.eq_ignore_ascii_case(aumid)) {
                let executable = application.GetStringValue(windows::core::w!("Executable")).ok().and_then(|value| take_string(value))?;
                let executable = executable.replace('/', "\\");
                let inside = !executable.is_empty() && !is_absolute(&executable) && !executable.contains(':') && !executable.split('\\').any(|part| part == "..");
                return (inside && is_exe(&executable)).then_some(executable);
            }
            let _ = applications.MoveNext().ok()?;
        }
        None
    }
}

/// A string the callee allocated with CoTaskMemAlloc, freed; None when null.
unsafe fn take_string(value: PWSTR) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let text = unsafe { value.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(value.0 as *const _)) };
    text
}

/// `System.Link.TargetParsingPath`, the program a `shell:AppsFolder` item that is a shortcut
/// points to.
const PKEY_LINK_TARGET_PARSING_PATH: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25), pid: 2 };

/// The executable the `shell:AppsFolder` item `name` points to, when it is an existing program.
fn shell_executable(name: &str) -> Result<String, (String, String)> {
    let parsing = windows_list::utf16(&format!(r"shell:AppsFolder\{name}"));
    let item: IShellItem2 = unsafe { SHCreateItemFromParsingName(PCWSTR(parsing.as_ptr()), None::<&IBindCtx>) }
        .map_err(|_| launch_failed("That app is no longer installed. Call computer_list_apps again."))?;
    link_program(&item).ok_or_else(|| launch_failed(NO_PROGRAM))
}

/// The existing program a `shell:AppsFolder` item that is a shortcut points to: the one rule both
/// `resolve_app_id` and `list_apps`' `program` (ticket 83) use.
fn link_program(item: &IShellItem2) -> Option<String> {
    let target = unsafe { item.GetString(&PKEY_LINK_TARGET_PARSING_PATH) }.ok().and_then(|value| unsafe { take_string(value) });
    match target.and_then(|target| classify(&target, &known_folder_path)) {
        Some(AppTarget::Exe(path)) if on_fixed_drive(&path, &drive_type) && Path::new(&path).is_file() => Some(path),
        _ => None,
    }
}

/// `DRIVE_FIXED` of `GetDriveTypeW` (its constant is in a `windows` feature this crate leaves off).
const DRIVE_FIXED: u32 = 3;

/// Whether `path` is `X:\...` on a fixed local drive, `drive_type` giving `GetDriveTypeW` of a root
/// like `C:\`. Anything else (UNC, `\\?\`, a mapped, removable or missing drive) is never looked
/// at, so reading a link's target cannot wait on the network (ticket 83 review).
fn on_fixed_drive(path: &str, drive_type: &dyn Fn(&str) -> u32) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' && drive_type(&path[..3]) == DRIVE_FIXED
}

fn drive_type(root: &str) -> u32 {
    let wide = windows_list::utf16(root);
    unsafe { GetDriveTypeW(PCWSTR(wide.as_ptr())) }
}

/// `(display name, parsing name, item)` of the items of the virtual `shell:AppsFolder`.
fn apps_folder_items() -> Vec<(String, String, IShellItem)> {
    let mut items = Vec::new();
    unsafe {
        let folder: IShellItem = match SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) {
            Ok(folder) => folder,
            Err(error) => {
                eprintln!("list_apps: shell:AppsFolder could not be opened: {error}");
                return items;
            }
        };
        let listing: IEnumShellItems = match folder.BindToHandler(None::<&IBindCtx>, &BHID_EnumItems) {
            Ok(listing) => listing,
            Err(error) => {
                eprintln!("list_apps: shell:AppsFolder could not be listed: {error}");
                return items;
            }
        };
        loop {
            let mut slot: [Option<IShellItem>; 1] = [None];
            let mut fetched = 0u32;
            let _ = listing.Next(&mut slot, Some(&mut fetched));
            let Some(item) = slot[0].take().filter(|_| fetched > 0) else {
                break;
            };
            if let Some(parsing_name) = item_name(&item, SIGDN_PARENTRELATIVEPARSING) {
                items.push((item_name(&item, SIGDN_NORMALDISPLAY).unwrap_or_default(), parsing_name, item));
            }
        }
    }
    items
}

fn item_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let name = item.GetDisplayName(kind).ok()?;
        let text = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        text
    }
}

/// The folder a `{KNOWNFOLDERID}` names.
fn known_folder_path(id: &str) -> Option<String> {
    let guid = GUID::try_from(id.trim_start_matches('{').trim_end_matches('}')).ok()?;
    folder_path(&guid)
}

fn folder_path(id: &GUID) -> Option<String> {
    unsafe {
        let path = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text
    }
}

/// The Start Menu Programs folders of the user and of the machine, or the test override.
fn start_menu_dirs() -> Vec<PathBuf> {
    if let Ok(dirs) = std::env::var(TEST_START_MENU_DIRS) {
        return dirs.split(';').filter(|dir| !dir.is_empty()).map(PathBuf::from).collect();
    }
    [&FOLDERID_Programs, &FOLDERID_CommonPrograms].into_iter().filter_map(folder_path).map(PathBuf::from).collect()
}

/// `(name, program)` of every shortcut under `dir` that starts a program: the shortcut's file name
/// without `.lnk`, and the executable it points to. Shortcuts to anything else are not apps.
fn shortcuts(dir: &Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    collect_shortcuts(dir, 0, &mut found);
    found
}

fn collect_shortcuts(dir: &Path, depth: usize, found: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if depth < 8 {
                collect_shortcuts(&path, depth + 1, found);
            }
        } else if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("lnk")) {
            let name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default().to_string();
            if let Some(program) = shortcut_target(&path).filter(|target| is_absolute(target) && is_exe(target)) {
                found.push((name, program));
            }
        }
    }
}

/// The path a shortcut points to, read without resolving (no search, no dialog).
fn shortcut_target(shortcut: &Path) -> Option<String> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let wide = windows_list::utf16(shortcut.to_str()?);
        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
        let mut buffer = [0u16; 1024];
        let mut data = WIN32_FIND_DATAW::default();
        link.GetPath(&mut buffer, &mut data, 0).ok()?;
        let length = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}

/// `(ROT13 value name, run count, last run)` of the UserAssist values under `root` (an HKCU
/// subkey), every `{GUID}\Count` key. Only records of the 72-byte format with a run or a time.
fn user_assist(root: &str) -> Vec<(String, u32, u64)> {
    let mut records = Vec::new();
    let Some(root_key) = RegKey::open(root) else {
        return records;
    };
    for guid in root_key.subkeys() {
        let Some(count_key) = RegKey::open(&format!(r"{root}\{guid}\Count")) else {
            continue;
        };
        for (name, data) in count_key.binary_values() {
            if let Some((count, last_run)) = parse_record(&data).filter(|(count, last_run)| *count != 0 || *last_run != 0) {
                records.push((name, count, last_run));
            }
        }
    }
    records
}

struct RegKey(HKEY);

impl RegKey {
    fn open(path: &str) -> Option<Self> {
        let wide = windows_list::utf16(path);
        let mut key = HKEY::default();
        let opened = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()), None, KEY_READ, &mut key) };
        (opened == ERROR_SUCCESS).then_some(Self(key))
    }

    fn subkeys(&self) -> Vec<String> {
        let mut names = Vec::new();
        for index in 0.. {
            let mut buffer = [0u16; 256];
            let mut length = buffer.len() as u32;
            let result = unsafe { RegEnumKeyExW(self.0, index, Some(PWSTR(buffer.as_mut_ptr())), &mut length, None, None, None, None) };
            if result == ERROR_MORE_DATA {
                continue;
            }
            if result != ERROR_SUCCESS {
                break;
            }
            names.push(String::from_utf16_lossy(&buffer[..length as usize]));
        }
        names
    }

    /// `(name, data)` of the REG_BINARY values; a value too large for the buffers is skipped.
    fn binary_values(&self) -> Vec<(String, Vec<u8>)> {
        let mut values = Vec::new();
        for index in 0.. {
            let mut name = vec![0u16; 2048];
            let mut name_length = name.len() as u32;
            let mut data = vec![0u8; 512];
            let mut data_length = data.len() as u32;
            let mut kind = 0u32;
            let result = unsafe {
                RegEnumValueW(self.0, index, Some(PWSTR(name.as_mut_ptr())), &mut name_length, None, Some(&mut kind), Some(data.as_mut_ptr()), Some(&mut data_length))
            };
            if result == ERROR_MORE_DATA {
                continue;
            }
            if result != ERROR_SUCCESS {
                break;
            }
            if kind == REG_BINARY.0 {
                data.truncate(data_length as usize);
                values.push((String::from_utf16_lossy(&name[..name_length as usize]), data));
            }
        }
        values
    }
}

impl Drop for RegKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

/// What the processes of the open windows are: their executables, and for a packaged app its
/// AppUserModelID (also for a UWP app's frame, whose app is the one inside it, ticket 34).
fn running_targets() -> Vec<AppTarget> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for (path, pid) in windows_list::listed_app_processes() {
        if !seen.insert(pid) {
            continue;
        }
        targets.push(AppTarget::Exe(path));
        if let Some(aumid) = process_aumid(pid) {
            targets.push(AppTarget::Uwp(aumid));
        }
    }
    targets
}

/// The AppUserModelID of a packaged app's process; None for a plain desktop process.
fn process_aumid(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 256];
        let mut length = buffer.len() as u32;
        let result = GetApplicationUserModelId(process, &mut length, Some(PWSTR(buffer.as_mut_ptr())));
        let _ = CloseHandle(process);
        // The length counts the terminating null.
        (result == ERROR_SUCCESS && length > 1).then(|| String::from_utf16_lossy(&buffer[..length as usize - 1]))
    }
}

/// How an app is started, and the one place that turns it into the `app_id` the server gets back.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AppTarget {
    /// A packaged app, by its AppUserModelID (`Family!App`).
    Uwp(String),
    /// A desktop program, by the absolute path of its executable.
    Exe(String),
    /// Anything the shell can start by its parsing name (`shell:AppsFolder\<name>`).
    Shell(String),
}

impl AppTarget {
    fn app_id(&self) -> String {
        match self {
            AppTarget::Uwp(aumid) => format!("uwp:{aumid}"),
            AppTarget::Exe(path) => format!("exe:{path}"),
            AppTarget::Shell(name) => format!("shell:{name}"),
        }
    }

    /// Ticket 79: the target an `app_id` names, read back exactly as `app_id` writes it (the form in
    /// any case). None for anything `list_apps` never gives: another form, a `uwp:` id that is not
    /// one `Family!App`, an `exe:` path that is not an absolute `.exe`, a `shell:` name with a path
    /// in it, control characters.
    fn from_app_id(app_id: &str) -> Option<AppTarget> {
        let (form, rest) = app_id.split_once(':')?;
        if rest.is_empty() || rest.chars().any(char::is_control) {
            return None;
        }
        let pathless = |text: &str| !text.is_empty() && !text.contains(['\\', '/', ':']);
        match form.to_ascii_lowercase().as_str() {
            "uwp" => {
                let (family, app) = rest.split_once('!')?;
                (pathless(family) && pathless(app) && !app.contains('!')).then(|| AppTarget::Uwp(rest.to_string()))
            }
            "exe" => (is_absolute(rest) && is_exe(rest)).then(|| AppTarget::Exe(rest.to_string())),
            "shell" => pathless(rest).then(|| AppTarget::Shell(rest.to_string())),
            _ => None,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            AppTarget::Uwp(_) => "uwp",
            AppTarget::Exe(_) | AppTarget::Shell(_) => "desktop",
        }
    }

    /// Two spellings of the same app (paths and ids compare without regard to case) share a key.
    fn key(&self) -> String {
        self.app_id().to_lowercase()
    }
}

/// What `name` (a `shell:AppsFolder` parsing name or a decoded UserAssist value name) starts.
/// `known_folder` turns the `{KNOWNFOLDERID}` of the `{KNOWNFOLDERID}\rest` form into its path.
fn classify(name: &str, known_folder: &dyn Fn(&str) -> Option<String>) -> Option<AppTarget> {
    if name.is_empty() || name.starts_with("UEME_") {
        return None;
    }
    if let Some(rest) = name.strip_prefix('{') {
        if let Some((id, tail)) = rest.split_once("}\\") {
            return Some(match known_folder(&format!("{{{id}}}")) {
                Some(folder) if is_exe(tail) => AppTarget::Exe(format!("{}\\{tail}", folder.trim_end_matches('\\'))),
                _ => AppTarget::Shell(name.to_string()),
            });
        }
    }
    let has_path_chars = name.contains('\\') || name.contains(':');
    if name.contains('!') && !has_path_chars {
        return Some(AppTarget::Uwp(name.to_string()));
    }
    if is_absolute(name) && is_exe(name) {
        return Some(AppTarget::Exe(name.to_string()));
    }
    Some(AppTarget::Shell(name.to_string()))
}

/// Whether a `shell:AppsFolder` item is an app. The folder also holds the Start Menu's help files
/// and links, which are items named by a path to something other than a program.
fn is_app(target: &AppTarget) -> bool {
    !matches!(target, AppTarget::Shell(name) if name.contains('\\') || name.contains(':'))
}

/// The Windows explorer.exe. Shortcuts to it, `exe:` or `shell:`, open a folder (the SDK's, say),
/// so they are not apps; File Explorer itself is listed as `shell:Microsoft.Windows.Explorer`,
/// which names no program (ticket 83).
fn windows_explorer() -> Option<String> {
    folder_path(&FOLDERID_Windows).map(|windows| format!(r"{}\explorer.exe", windows.trim_end_matches('\\')))
}

/// Whether `target` (a `shell:` one running `program`) is a shortcut to `explorer`, which opens a
/// folder. `shell:Microsoft.Windows.Explorer` is File Explorer itself and always stays.
fn is_folder_link(target: &AppTarget, program: Option<&str>, explorer: &str) -> bool {
    let is_explorer = |path: &str| !explorer.is_empty() && path.eq_ignore_ascii_case(explorer);
    match target {
        AppTarget::Shell(name) if name.eq_ignore_ascii_case("Microsoft.Windows.Explorer") => false,
        AppTarget::Shell(_) => program.is_some_and(is_explorer),
        AppTarget::Exe(path) => is_explorer(path),
        AppTarget::Uwp(_) => false,
    }
}

fn is_exe(path: &str) -> bool {
    path.len() > 4 && path.as_bytes()[path.len() - 4..].eq_ignore_ascii_case(b".exe")
}

fn is_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\") || (bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
}

fn rot13(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
            other => other,
        })
        .collect()
}

/// A Windows 7+ UserAssist record: the run count (offset 4) and the last run as a FILETIME
/// (offset 60) of its 72 bytes.
fn parse_record(data: &[u8]) -> Option<(u32, u64)> {
    if data.len() != 72 {
        return None;
    }
    let count = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let last_run = u64::from_le_bytes(data[60..68].try_into().ok()?);
    Some((count, last_run))
}

/// A FILETIME (100 ns since 1601-01-01 UTC) as `YYYY-MM-DDTHH:MM:SSZ`.
fn filetime_iso(filetime: u64) -> String {
    let seconds = filetime / 10_000_000;
    // 1601-01-01 is 134774 days before the Unix epoch; then the civil-from-days algorithm.
    let z = (seconds / 86_400) as i64 - 134_774 + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let in_day = seconds % 86_400;
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", in_day / 3_600, in_day % 3_600 / 60, in_day % 60)
}

/// One app as the sources report it, merged.
#[derive(Debug, Clone, PartialEq, Eq)]
struct App {
    target: AppTarget,
    name: String,
    /// The last run as a FILETIME.
    last_used: Option<u64>,
    use_count: Option<u32>,
    running: bool,
    /// For a `shell:` app, the executable it runs, when that is known (ticket 83).
    program: Option<String>,
}

/// The apps found so far, one per app: sources that list apps `add` them, sources that only know
/// something about an app (UserAssist, the running processes) `used` / `running` the ones already
/// there.
#[derive(Default)]
struct Catalog {
    apps: std::collections::HashMap<String, App>,
}

impl Catalog {
    fn add(&mut self, target: AppTarget, name: &str) {
        match self.apps.entry(target.key()) {
            std::collections::hash_map::Entry::Occupied(mut found) => {
                if found.get().name.is_empty() {
                    found.get_mut().name = name.to_string();
                }
            }
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(App { target, name: name.to_string(), last_used: None, use_count: None, running: false, program: None });
            }
        }
    }

    fn program(&mut self, target: &AppTarget, program: String) {
        if let Some(app) = self.apps.get_mut(&target.key()) {
            app.program.get_or_insert(program);
        }
    }

    fn used(&mut self, target: &AppTarget, count: u32, last_run: u64) {
        if let Some(app) = self.apps.get_mut(&target.key()) {
            app.merge_use(Some(count), (last_run != 0).then_some(last_run));
        }
    }

    fn running(&mut self, target: &AppTarget) {
        if let Some(app) = self.apps.get_mut(&target.key()) {
            app.running = true;
        }
    }

    /// The apps newest-used first (those never used after them, by name) cut to `limit`, and how
    /// many there were before the cut.
    fn finish(mut self, limit: usize) -> (Vec<App>, usize) {
        self.fold_shell_duplicates();
        let mut apps: Vec<App> = self.apps.into_values().collect();
        apps.sort_by(|a, b| {
            let by_use = match (a.last_used, b.last_used) {
                (Some(a), Some(b)) => b.cmp(&a),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            by_use
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.target.key().cmp(&b.target.key()))
        });
        let total = apps.len();
        apps.truncate(limit);
        (apps, total)
    }

    /// An app the shell lists only by a bare parsing name (a shortcut with its own AppUserModelID,
    /// like Office's) is often also a program found by its shortcut: the two are one app when their
    /// names are the same, and the program is the entry that stays.
    fn fold_shell_duplicates(&mut self) {
        let mut programs: Vec<(String, String)> = self
            .apps
            .iter()
            .filter(|(_, app)| !matches!(app.target, AppTarget::Shell(_)))
            .map(|(key, app)| (app.name.to_lowercase(), key.clone()))
            .collect();
        programs.sort();
        let shell_keys: Vec<String> =
            self.apps.iter().filter(|(_, app)| matches!(app.target, AppTarget::Shell(_))).map(|(key, _)| key.clone()).collect();
        for key in shell_keys {
            let name = self.apps[&key].name.to_lowercase();
            let Some((_, program)) = programs.iter().find(|(program_name, _)| *program_name == name) else {
                continue;
            };
            if let Some(shell) = self.apps.remove(&key) {
                if let Some(app) = self.apps.get_mut(program) {
                    app.merge_use(shell.use_count, shell.last_used);
                    app.running |= shell.running;
                }
            }
        }
    }
}

impl App {
    fn to_json(&self) -> Value {
        let mut app = json!({ "app_id": self.target.app_id(), "name": self.display_name(), "kind": self.target.kind(), "running": self.running });
        if let Some(last_used) = self.last_used {
            app["last_used"] = json!(filetime_iso(last_used));
        }
        if let Some(use_count) = self.use_count {
            app["use_count"] = json!(use_count);
        }
        if let Some(program) = &self.program {
            app["program"] = json!(program);
        }
        app
    }

    /// The name the sources gave, or when none did, what the app is started by.
    fn display_name(&self) -> String {
        if !self.name.is_empty() {
            return self.name.clone();
        }
        match &self.target {
            AppTarget::Exe(path) => Path::new(path).file_stem().and_then(|stem| stem.to_str()).unwrap_or(path).to_string(),
            AppTarget::Uwp(id) | AppTarget::Shell(id) => id.clone(),
        }
    }

    /// Counts add up and the later last run wins, as the same app run under two spellings.
    fn merge_use(&mut self, count: Option<u32>, last_run: Option<u64>) {
        self.use_count = match (self.use_count, count) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (a, b) => a.or(b),
        };
        self.last_used = self.last_used.max(last_run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exe(path: &str) -> AppTarget {
        AppTarget::Exe(path.to_string())
    }

    #[test]
    fn one_entry_per_app_with_the_first_name_and_everything_the_sources_know() {
        let mut catalog = Catalog::default();
        catalog.add(exe(r"C:\Tools\Edit.exe"), "Editor");
        catalog.add(exe(r"c:\tools\edit.EXE"), "Edit (shortcut)");
        catalog.add(exe(r"C:\Tools\Other.exe"), "Other");
        catalog.used(&exe(r"C:\TOOLS\edit.exe"), 3, 100);
        catalog.used(&exe(r"C:\tools\edit.exe"), 4, 90);
        catalog.running(&exe(r"c:\tools\EDIT.EXE"));
        // Enrichment never lists an app nobody listed.
        catalog.used(&exe(r"C:\Gone.exe"), 9, 5);
        catalog.running(&exe(r"C:\Unlisted.exe"));
        let (apps, total) = catalog.finish(200);
        assert_eq!(total, 2);
        let edit = apps.iter().find(|app| app.name == "Editor").expect("Editor");
        assert_eq!((edit.use_count, edit.last_used, edit.running), (Some(7), Some(100), true));
        let other = apps.iter().find(|app| app.name == "Other").expect("Other");
        assert_eq!((other.use_count, other.last_used, other.running), (None, None, false));
    }

    #[test]
    fn a_shell_only_entry_is_folded_into_the_program_of_the_same_name() {
        let mut catalog = Catalog::default();
        catalog.add(AppTarget::Shell("Microsoft.Office.WINWORD.EXE.15".into()), "Word");
        catalog.add(exe(r"C:\Office\WINWORD.EXE"), "word");
        catalog.add(AppTarget::Shell("Lonely.App".into()), "Lonely");
        catalog.used(&AppTarget::Shell("Microsoft.Office.WINWORD.EXE.15".into()), 2, 50);
        catalog.running(&AppTarget::Shell("microsoft.office.winword.exe.15".into()));
        let (apps, total) = catalog.finish(200);
        assert_eq!(total, 2);
        let word = apps.iter().find(|app| app.target == exe(r"C:\Office\WINWORD.EXE")).expect("the program");
        assert_eq!((word.use_count, word.last_used, word.running), (Some(2), Some(50), true));
        assert!(apps.iter().any(|app| app.target == AppTarget::Shell("Lonely.App".into())));
    }

    #[test]
    fn apps_are_sorted_newest_used_first_then_the_unused_by_name() {
        let mut catalog = Catalog::default();
        for (path, name) in [("a", "zeta"), ("b", "Beta"), ("c", "alpha"), ("d", "Old"), ("e", "New")] {
            catalog.add(exe(&format!(r"C:\{path}.exe")), name);
        }
        catalog.used(&exe(r"C:\d.exe"), 1, 10);
        catalog.used(&exe(r"C:\e.exe"), 1, 20);
        let (apps, _) = catalog.finish(200);
        let names: Vec<&str> = apps.iter().map(|app| app.name.as_str()).collect();
        assert_eq!(names, ["New", "Old", "alpha", "Beta", "zeta"]);
    }

    #[test]
    fn the_list_is_cut_to_the_limit_and_the_total_still_counts_every_app() {
        let mut catalog = Catalog::default();
        for number in 0..5 {
            catalog.add(exe(&format!(r"C:\app{number}.exe")), &format!("app{number}"));
        }
        let (apps, total) = catalog.finish(3);
        assert_eq!((apps.len(), total), (3, 5));
        assert_eq!(apps[0].name, "app0");
    }

    #[test]
    fn start_menu_entries_carry_a_package_key_a_program_key_or_none() {
        let mut catalog = Catalog::default();
        catalog.add(AppTarget::Uwp("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App".into()), "終端機");
        catalog.add(exe(r"C:\Tools\Signed.exe"), "Signed");
        catalog.add(exe(r"C:\Tools\Plain.EXE"), "Plain");
        catalog.add(exe(r"\\server\share\Remote.exe"), "Remote");
        catalog.add(AppTarget::Shell("Microsoft.AutoGenerated.{8E66}".into()), "Settings");
        catalog.program(&AppTarget::Shell("Microsoft.AutoGenerated.{8E66}".into()), r"C:\Tools\Signed.exe".into());
        catalog.add(AppTarget::Shell("Microsoft.Windows.Explorer".into()), "檔案總管");
        let (apps, _) = catalog.finish(usize::MAX);
        let entries: Vec<StartMenuEntry> = apps.iter().map(StartMenuEntry::of).collect();
        // As `program_keys` answers: a signer's key, a path's, and none for a program off the fixed drives.
        let signed = |program: &str| match program {
            _ if program.eq_ignore_ascii_case(r"C:\Tools\Signed.exe") => Some("signer:CN=Contoso|signed.exe".to_string()),
            _ if program.starts_with(r"\\") => None,
            _ => Some(crate::identity::path_key(program)),
        };
        let listed = start_menu_json(&entries, &signed);
        let row = |name: &str| listed["apps"].as_array().unwrap().iter().find(|app| app["name"] == name).cloned().unwrap();
        assert_eq!(row("終端機"), json!({ "app_id": "uwp:Microsoft.WindowsTerminal_8wekyb3d8bbwe!App", "name": "終端機", "key": "package:microsoft.windowsterminal_8wekyb3d8bbwe" }));
        assert_eq!(row("Signed")["key"], "signer:CN=Contoso|signed.exe");
        assert_eq!(row("Settings"), json!({ "app_id": "shell:Microsoft.AutoGenerated.{8E66}", "name": "Settings", "key": "signer:CN=Contoso|signed.exe", "program": r"C:\Tools\Signed.exe" }));
        // An unsigned program is known by its path, as the approval check knows it.
        assert_eq!(row("Plain")["key"], r"c:\tools\plain.exe");
        assert_eq!(row("檔案總管"), json!({ "app_id": "shell:Microsoft.Windows.Explorer", "name": "檔案總管", "key": null }));
        assert_eq!(row("Remote"), json!({ "app_id": r"exe:\\server\share\Remote.exe", "name": "Remote", "key": null, "program": r"\\server\share\Remote.exe" }));
        assert_eq!(listed["apps"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn only_programs_on_a_fixed_drive_are_opened_for_their_identity_each_once() {
        let drives = |root: &str| match root {
            r"C:\" => 3, // DRIVE_FIXED
            r"Z:\" => 4, // DRIVE_REMOTE
            _ => 1,
        };
        let entry = |program: &str| StartMenuEntry { app_id: format!("exe:{program}"), name: "x".into(), family: None, program: Some(program.to_string()) };
        let entries = [entry(r"C:\Tools\a.exe"), entry(r"c:\tools\A.EXE"), entry(r"\\server\share\b.exe"), entry(r"Z:\mapped\c.exe"), entry(r"C:\Tools\d.exe")];
        assert_eq!(programs_to_identify(&entries, &drives), [r"C:\Tools\a.exe", r"C:\Tools\d.exe"]);
    }

    fn no_known_folders(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn rot13_decodes_letters_and_leaves_everything_else() {
        assert_eq!(rot13("P:\\Cebtenz Svyrf\\Abgrcnq.rkr"), "C:\\Program Files\\Notepad.exe");
        assert_eq!(rot13(&rot13("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")), "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        assert_eq!(rot13("HRZR_PGYFRFFVBA"), "UEME_CTLSESSION");
        assert_eq!(rot13(""), "");
    }

    fn record(count: u32, filetime: u64, len: usize) -> Vec<u8> {
        let mut data = vec![0u8; 72];
        data[4..8].copy_from_slice(&count.to_le_bytes());
        data[60..68].copy_from_slice(&filetime.to_le_bytes());
        data.resize(len, 0);
        data
    }

    #[test]
    fn a_72_byte_record_gives_its_run_count_and_last_run() {
        assert_eq!(parse_record(&record(7, 133_500_000_000_000_000, 72)), Some((7, 133_500_000_000_000_000)));
        assert_eq!(parse_record(&record(0, 0, 72)), Some((0, 0)));
    }

    #[test]
    fn a_record_of_another_size_is_not_read() {
        assert_eq!(parse_record(&record(7, 1, 16)), None);
        assert_eq!(parse_record(&record(7, 1, 71)), None);
        assert_eq!(parse_record(&record(7, 1, 73)), None);
        assert_eq!(parse_record(&[]), None);
    }

    #[test]
    fn a_filetime_is_an_iso_date_in_utc() {
        assert_eq!(filetime_iso(116_444_736_000_000_000), "1970-01-01T00:00:00Z");
        // 2024-02-29 12:34:56 UTC
        assert_eq!(filetime_iso(133_536_836_960_000_000), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn a_shell_item_that_is_a_file_rather_than_a_program_is_not_an_app() {
        // Help files, links and other documents that a Start Menu folder also holds.
        assert!(!is_app(&AppTarget::Shell(r"{6D809377-6AF0-444B-8957-A3773F02200E}\7-Zip\7-zip.chm".into())));
        assert!(!is_app(&AppTarget::Shell(r"C:\Docs\readme.txt".into())));
        assert!(is_app(&AppTarget::Shell("Microsoft.Windows.Explorer".into())));
        assert!(is_app(&AppTarget::Exe(r"C:\Windows\notepad.exe".into())));
        assert!(is_app(&AppTarget::Uwp("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into())));
    }

    #[test]
    fn only_a_path_on_a_fixed_local_drive_is_looked_at_as_a_program() {
        // Ticket 83 review: list_apps reads every shell: entry's target, so a network or removable
        // path (an SMB timeout, a drive not there) is never touched.
        let drives = |root: &str| match root {
            r"C:\" | r"d:\" => DRIVE_FIXED,
            r"Z:\" => 4, // DRIVE_REMOTE
            r"E:\" => 2, // DRIVE_REMOVABLE
            _ => 1,      // DRIVE_NO_ROOT_DIR
        };
        assert!(on_fixed_drive(r"C:\Windows\System32\taskmgr.exe", &drives));
        assert!(on_fixed_drive(r"d:\Tools\x.exe", &drives));
        for path in [r"\\server\share\x.exe", r"\\?\C:\x.exe", r"Z:\mapped\x.exe", r"E:\usb\x.exe", r"Q:\gone\x.exe", r"C:x.exe", "x.exe", ""] {
            assert!(!on_fixed_drive(path, &drives), "{path:?}");
        }
    }

    #[test]
    fn a_link_to_the_windows_explorer_is_a_folder_link_but_file_explorer_itself_is_not() {
        let explorer = r"C:\Windows\explorer.exe";
        let program = Some(r"C:\WINDOWS\explorer.exe");
        assert!(is_folder_link(&exe(r"c:\windows\EXPLORER.EXE"), None, explorer));
        assert!(is_folder_link(&AppTarget::Shell("Microsoft.AutoGenerated.{3B6E7DF3-19A3-70DB-96F6-86E65A17E61C}".into()), program, explorer));
        assert!(!is_folder_link(&AppTarget::Shell("Microsoft.Windows.Explorer".into()), program, explorer));
        assert!(!is_folder_link(&AppTarget::Shell("microsoft.windows.explorer".into()), program, explorer));
        assert!(!is_folder_link(&AppTarget::Shell("Microsoft.Windows.Explorer".into()), None, explorer));
        assert!(!is_folder_link(&exe(r"C:\Windows\notepad.exe"), None, explorer));
        assert!(!is_folder_link(&AppTarget::Shell("Other".into()), Some(r"C:\Windows\notepad.exe"), explorer));
    }

    #[test]
    fn the_app_id_names_how_the_app_is_started() {
        assert_eq!(AppTarget::Uwp("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()).app_id(), "uwp:Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
        assert_eq!(AppTarget::Exe(r"C:\Windows\notepad.exe".into()).app_id(), r"exe:C:\Windows\notepad.exe");
        assert_eq!(AppTarget::Shell("Microsoft.Office.WINWORD.EXE.15".into()).app_id(), "shell:Microsoft.Office.WINWORD.EXE.15");
    }

    #[test]
    fn an_app_id_reads_back_as_the_target_it_was_made_from_and_nothing_else_does() {
        for target in [
            AppTarget::Uwp("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()),
            AppTarget::Exe(r"C:\Windows\notepad.exe".into()),
            AppTarget::Exe(r"\\server\share\tool.EXE".into()),
            AppTarget::Shell("Microsoft.Office.WINWORD.EXE.15".into()),
        ] {
            assert_eq!(AppTarget::from_app_id(&target.app_id()), Some(target.clone()), "{target:?}");
        }
        assert_eq!(AppTarget::from_app_id(r"EXE:C:\Windows\notepad.exe"), Some(exe(r"C:\Windows\notepad.exe")));
        for app_id in [
            "",
            "notepad",
            "foo:bar",
            "uwp:",
            "uwp:NoBang",
            "uwp:A!B!C",
            "uwp:!App",
            r"uwp:C:\x!App",
            "exe:notepad.exe",
            r"exe:C:\Docs\readme.txt",
            "shell:",
            r"shell:C:\Windows\System32\cmd.exe",
            r"shell:..\x",
            "shell:a/b",
            "shell:line\nbreak",
        ] {
            assert_eq!(AppTarget::from_app_id(app_id), None, "{app_id:?}");
        }
    }

    #[test]
    fn names_are_classified_as_aumid_exe_path_known_folder_path_or_shell_name() {
        let system = |id: &str| (id == "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}").then(|| r"C:\Windows\System32".to_string());
        assert_eq!(
            classify("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", &no_known_folders),
            Some(AppTarget::Uwp("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()))
        );
        assert_eq!(classify(r"C:\Tools\a.EXE", &no_known_folders), Some(AppTarget::Exe(r"C:\Tools\a.EXE".into())));
        assert_eq!(classify(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe", &system), Some(AppTarget::Exe(r"C:\Windows\System32\notepad.exe".into())));
        // A known folder that does not resolve, a path that is not a program, a bare name.
        assert_eq!(
            classify(r"{00000000-0000-0000-0000-000000000000}\x.exe", &system),
            Some(AppTarget::Shell(r"{00000000-0000-0000-0000-000000000000}\x.exe".into()))
        );
        assert_eq!(classify(r"C:\Docs\readme.txt", &no_known_folders), Some(AppTarget::Shell(r"C:\Docs\readme.txt".into())));
        assert_eq!(classify("Microsoft.Office.WINWORD.EXE.15", &no_known_folders), Some(AppTarget::Shell("Microsoft.Office.WINWORD.EXE.15".into())));
        // UserAssist's own bookkeeping is not an app.
        assert_eq!(classify("UEME_CTLSESSION", &no_known_folders), None);
        assert_eq!(classify("", &no_known_folders), None);
    }
}
