//! Filesystem paths in Windows are a total mess. This crate normalizes paths to the most
//! compatible (but still correct) format, so that you don't have to worry about the mess.
//!
//! In Windows the regular/legacy paths (`C:\foo`) are supported by all programs, but have
//! lots of bizarre restrictions for backwards compatibility with MS-DOS.
//!
//! And there are Windows NT UNC paths (`\\?\C:\foo`), which are more robust and with fewer
//! gotchas, but are rarely supported by Windows programs. Even Microsoft's own!
//!
//! This crate converts paths to legacy format whenever possible, but leaves UNC paths as-is
//! when they can't be unambiguously expressed in a simpler way. This allows legacy programs
//! to access all paths they can possibly access, and UNC-aware programs to access all paths.
//!
//! On non-Windows platforms these functions leave paths unmodified, so it's safe to use them
//! unconditionally for all platforms.
//!
//! Parsing is based on <https://msdn.microsoft.com/en-us/library/windows/desktop/aa365247(v=vs.85).aspx>
//!
//! [Project homepage](https://lib.rs/crates/dunce).
#![doc(
    html_logo_url = "https://assets.gitlab-static.net/uploads/-/system/project/avatar/4717715/dyc.png"
)]

// spell-checker:ignore (abbrev/names) NTFS (people) rivy () canonicalize canonicalization losslessly pathing realpath CONIN CONOUT GLOBALROOT notdisk serv

// ToDO: [2023-05-29; rivy] change `wild` to return any reserved-word file name with a './' prefix (single [or multiple] trailing dots don't work b/c they can be legal NTFS filenames)
// ... then the code here will correctly create a UNC path for it, as needed
// NOTES: `std::fs::absolute` is now available to help with logical/virtual path canonicalization/resolution.
// ... if there are no '..' segments in a path, then absolute should always be correct
// ... reference the double-dot problem article (<https://9p.io/sys/doc/lexnames.html> @@ <https://archive.is/vF9QF> , <https://archive.is/xtuYp>)
// ... see further research from <https://workona.com/0/j4j2n9/paths-rust-unc-research> and <https://workona.com/0/uaxc07/path-canonicalization-and-traversal-toctou-and-races>
// development a hybrid canonicalization strategy that accesses the file system until the file doesn't exist then uses purely lexical/virtual reasoning
// ? canonicalization is difficult if the current path is a symbolic link (must be resolved to a physical path), then each level must be resolved

// see: kb-pathing.mkd, kb-Unicode-UNC-&-portable-paths.mkd, kb-Win32+NT-Paths.mkd
// ref: - [MSDN - Windows: Naming Files, Paths, and Namespaces](http://msdn.microsoft.com/en-us/library/windows/desktop/aa365247(v=vs.85).aspx) @@ <https://archive.today/DgH7i>

// #[cfg(any(windows, test))]
use std::ffi::OsStr;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt;
#[cfg(windows)]
use std::path::{Component, Prefix};
use std::path::{Path, PathBuf};

/// Takes any path, and when possible, converts Windows UNC paths to regular paths.
/// If the path can't be converted, it's returned unmodified.
///
/// On non-Windows this is no-op.
///
/// `\\?\C:\Windows` will be converted to `C:\Windows`,
/// but `\\?\C:\COM` will be left as-is (due to a reserved filename).
///
/// Use this to pass arbitrary paths to programs that may not be UNC-aware.
///
/// It's generally safe to pass UNC paths to legacy programs, because
/// these paths contain a reserved prefix, so will gracefully fail
/// if used with legacy APIs that don't support UNC.
///
/// This function does not perform any I/O.
///
/// Currently paths with unpaired surrogates aren't converted even if they
/// could be, due to limitations of Rust's `OsStr` API.
///
/// To check if a path remained as UNC, use [`is_simplified()`] or `path.as_os_str().as_encoded_bytes().starts_with(b"\\\\")`.
#[inline]
#[must_use]
pub fn simplified(path: &Path) -> &Path {
    try_simplified(path).unwrap_or(path)
}

/// Like `std::fs::canonicalize()`, but on Windows it outputs the most
/// compatible form of a path instead of UNC.
#[inline(always)]
#[doc(alias = "realpath")]
pub fn canonicalize<P: AsRef<Path>>(path: P) -> io::Result<PathBuf> {
    let path = path.as_ref();

    #[cfg(not(windows))]
    {
        fs::canonicalize(path)
    }
    #[cfg(windows)]
    {
        canonicalize_win(path)
    }
}

// #[cfg(windows)]
// fn remove_trailing_dots(input: &Path) -> &Path {
//     let mut stripped = input;
//     while let Some(new_stripped) = stripped.strip_suffix(b".") {
//         stripped = new_stripped;
//     }
//     stripped
// }

pub fn strip_trailing_dots<P: AsRef<Path>>(path: P) -> std::ffi::OsString {
    #[cfg(unix)]
    {
        let bytes = path.as_ref().as_os_str().as_bytes();
        let new_bytes = bytes
            .iter()
            .rev()
            .skip_while(|&&b| b == b'.')
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        OsStr::from_bytes(&new_bytes).to_os_string()
    }

    #[cfg(windows)]
    {
        let wide_chars = path.as_ref().as_os_str().encode_wide().collect::<Vec<_>>();
        let new_wide_chars = wide_chars
            .iter()
            .rev()
            .skip_while(|&&c| c == '.' as u16)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        OsStringExt::from_wide(&new_wide_chars)
    }
}

#[cfg(windows)]
fn canonicalize_win(path: &Path) -> io::Result<PathBuf> {
    let reserved = is_reserved(path);
    let exact_reserve = RESERVED_NAMES.contains(&path.to_str().unwrap_or_default());

    if reserved && !exact_reserve {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid path"))?;
        let file = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid path"))?;

        let mut ret_path = PathBuf::from(format!(
            "\\\\?\\{}",
            canonicalize_win(match parent.to_str() {
                Some("") => Path::new("."),
                _ => parent,
            })?
            .display()
        ));
        ret_path.push(strip_trailing_dots(file));

        return Ok(ret_path);
    }

    let real_path = fs::canonicalize(path)?;
    Ok(try_simplified(&real_path).map(PathBuf::from).unwrap_or(real_path))
}

/// Returns `true` if the path is relative or starts with a disk prefix (like "C:").
#[cfg_attr(not(windows), allow(unused))]
#[must_use]
pub fn is_simplified(path: &Path) -> bool {
    #[cfg(windows)]
    if let Some(Component::Prefix(prefix)) = path.components().next() {
        return matches!(prefix.kind(), Prefix::Disk(..))
    }
    true
}
#[doc(hidden)]
pub use self::canonicalize as realpath;

#[cfg(any(windows, test))]
fn windows_char_len(s: &OsStr) -> usize {
    #[cfg(not(windows))]
    let len = s
        .to_string_lossy()
        .chars()
        .map(|c| if c as u32 <= 0xFFFF { 1 } else { 2 })
        .sum();
    #[cfg(windows)]
    let len = s.encode_wide().count();
    len
}

#[cfg(any(windows, test))]
fn is_valid_filename(file_name: &OsStr) -> bool {
    if file_name.len() > 255 && windows_char_len(file_name) > 255 {
        return false;
    }

    // Non-unicode is safe, but Rust can't reasonably losslessly operate on such strings
    let byte_str = if let Some(s) = file_name.to_str() {
        s.as_bytes()
    } else {
        return false;
    };
    if byte_str.is_empty() {
        return false;
    }
    // Only ASCII subset is checked, and WTF-8/UTF-8 is safe for that
    if byte_str.iter().any(|&c| matches!(c, 0..=31 | b'<' | b'>' | b':' | b'"' | b'/' | b'\\' | b'|' | b'?' | b'*')) {
        return false
    }
    // Filename can't end with . or space (except before extension, but this checks the whole name)
    if matches!(byte_str.last(), Some(b' ' | b'.')) {
        return false;
    }
    true
}

// ref:
#[cfg(any(windows, test))]
const RESERVED_NAMES: [&str; 24] = [
    "AUX", "NUL", "PRN", "CON", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9", "CONIN$",
    "CONOUT$",
];

#[cfg(any(windows, test))]
fn is_reserved<P: AsRef<OsStr>>(file_name: P) -> bool {
    // con.txt is reserved too
    // all reserved DOS names have ASCII-compatible stem
    if let Some(name) = Path::new(&file_name).file_stem().and_then(|s| s.to_str()?.split('.').next()) {
        // "con.. .txt" is "CON" for DOS
        let trimmed = name.trim_end_matches(' ');
        return trimmed.len() <= 4 && RESERVED_NAMES.into_iter().any(|name| trimmed.eq_ignore_ascii_case(name));
    }
    false
}

#[cfg(not(windows))]
#[inline]
const fn try_simplified(_path: &Path) -> Option<&Path> {
    None
}

#[cfg(windows)]
fn try_simplified(path: &Path) -> Option<&Path> {
    eprintln!("is_safe_to_strip_unc({:?})", path);
    let mut components = path.components();
    match components.next() {
        Some(Component::Prefix(p)) => match p.kind() {
            Prefix::VerbatimDisk(..) => {}
            _ => return None, // Other kinds of UNC paths
        },
        _ => return None, // relative or empty
    }
    let stripped_path = components.as_path();

    for component in components {
        match component {
            Component::RootDir => {}
            Component::Normal(file_name) => {
                if !is_valid_filename(file_name) || is_reserved(file_name) {
                    return None;
                }
            }
            _ => return None, // UNC paths take things like ".." literally
        };
    }

    let path_os_str = stripped_path.as_os_str();
    // However, if the path is going to be used as a directory it's 248
    if path_os_str.len() > 260 && windows_char_len(path_os_str) > 260 {
        return None;
    }
    Some(stripped_path)
}

#[test]
fn reserved() {
    assert!(is_reserved("CON"));
    assert!(is_reserved("con"));
    assert!(is_reserved("con.con"));
    assert!(is_reserved("COM4"));
    assert!(is_reserved("COM4.txt"));
    assert!(is_reserved("COM4 .txt"));
    assert!(is_reserved("con."));
    assert!(is_reserved("con ."));
    assert!(is_reserved("con  "));
    assert!(is_reserved("con . "));
    assert!(is_reserved("con . .txt"));
    assert!(is_reserved("con.....txt"));
    assert!(is_reserved("PrN....."));
    assert!(is_reserved("nul.tar.gz"));

    assert!(!is_reserved(" PrN....."));
    assert!(!is_reserved(" CON"));
    assert!(!is_reserved("COM0"));
    assert!(!is_reserved("COM77"));
    assert!(!is_reserved(" CON "));
    assert!(!is_reserved(".CON"));
    assert!(!is_reserved("@CON"));
    assert!(!is_reserved("not.CON"));
    assert!(!is_reserved("CON。"));
}

#[test]
fn len() {
    assert_eq!(1, windows_char_len(OsStr::new("a")));
    assert_eq!(1, windows_char_len(OsStr::new("€")));
    assert_eq!(1, windows_char_len(OsStr::new("本")));
    assert_eq!(2, windows_char_len(OsStr::new("🧐")));
    assert_eq!(2, windows_char_len(OsStr::new("®®")));
}

#[test]
fn valid() {
    assert!(!is_valid_filename("..".as_ref()));
    assert!(!is_valid_filename(".".as_ref()));
    assert!(!is_valid_filename("aaaaaaaaaa:".as_ref()));
    assert!(!is_valid_filename("ą:ą".as_ref()));
    assert!(!is_valid_filename("".as_ref()));
    assert!(!is_valid_filename("a ".as_ref()));
    assert!(!is_valid_filename(" a. ".as_ref()));
    assert!(!is_valid_filename("a/".as_ref()));
    assert!(!is_valid_filename("/a".as_ref()));
    assert!(!is_valid_filename("/".as_ref()));
    assert!(!is_valid_filename("\\".as_ref()));
    assert!(!is_valid_filename("\\a".as_ref()));
    assert!(!is_valid_filename("<x>".as_ref()));
    assert!(!is_valid_filename("a*".as_ref()));
    assert!(!is_valid_filename("?x".as_ref()));
    assert!(!is_valid_filename("a\0a".as_ref()));
    assert!(!is_valid_filename("\x1f".as_ref()));
    assert!(!is_valid_filename("a".repeat(257).as_ref()));

    assert!(is_valid_filename("®".repeat(254).as_ref()));
    assert!(is_valid_filename("ファイル".as_ref()));
    assert!(is_valid_filename("a".as_ref()));
    assert!(is_valid_filename("a.aaaaaaaa".as_ref()));
    assert!(is_valid_filename("a........a".as_ref()));
    assert!(is_valid_filename("       b".as_ref()));
}

#[test]
#[cfg(windows)]
fn realpath_test() {
    assert_eq!(
        r"C:\WINDOWS",
        canonicalize(r"C:\Windows")
            .unwrap()
            .to_str()
            .unwrap()
            .to_uppercase()
    );
    assert_ne!(r".", canonicalize(r".").unwrap().to_str().unwrap());
}

#[test]
#[cfg(windows)]
fn strip() {
    assert_eq!(
        Path::new(r"C:\foo\😀"),
        simplified(Path::new(r"\\?\C:\foo\😀"))
    );
    assert_eq!(Path::new(r"\\?\serv\"), simplified(Path::new(r"\\?\serv\")));
    assert_eq!(
        Path::new(r"\\.\C:\notdisk"),
        simplified(Path::new(r"\\.\C:\notdisk"))
    );
    assert_eq!(
        Path::new(r"\\?\GLOBALROOT\Device\ImDisk0\path\to\file.txt"),
        simplified(Path::new(r"\\?\GLOBALROOT\Device\ImDisk0\path\to\file.txt"))
    );
}

#[test]
#[cfg(windows)]
fn safe() {
    assert!(try_simplified(Path::new(r"\\?\C:\foo\bar")).is_some());
    assert!(try_simplified(Path::new(r"\\?\Z:\foo\bar\")).is_some());
    assert!(try_simplified(Path::new(r"\\?\Z:\😀\🎃\")).is_some());
    assert!(try_simplified(Path::new(r"\\?\c:\foo")).is_some());

    let long = ::std::iter::repeat("®").take(160).collect::<String>();
    assert!(try_simplified(Path::new(&format!(r"\\?\c:\{}", long))).is_some());
    assert!(!try_simplified(Path::new(&format!(r"\\?\c:\{}\{}", long, long))).is_some());

    assert!(!try_simplified(Path::new(r"\\?\C:\foo\.\bar")).is_some());
    assert!(!try_simplified(Path::new(r"\\?\C:\foo\..\bar")).is_some());
    assert!(!try_simplified(Path::new(r"\\?\c\foo")).is_some());
    assert!(!try_simplified(Path::new(r"\\?\c\foo/bar")).is_some());
    assert!(!try_simplified(Path::new(r"\\?\c:foo")).is_some());
    assert!(!try_simplified(Path::new(r"\\?\cc:foo")).is_some());
    assert!(!try_simplified(Path::new(r"\\?\c:foo\bar")).is_some());
}
