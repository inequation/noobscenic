//! Dropping root into the account named by `runas` — the `posix-runas` feature.
//!
//! A router or NAS rc script starts the server as root; `runas` in `config.json`
//! names the unprivileged account it should become instead (doc/PLAN.md §6). This
//! is the reference C snippet in Rust: the user is resolved with `getpwnam_r` (uid
//! plus the primary gid used when `runas.group` is absent), an explicit group with
//! `getgrnam_r`, and the switch is `setgid` → `initgroups` → `setuid`.
//!
//! `main` calls this before the log file and the data directory are created, so
//! everything the server writes afterwards belongs to the target account. Names
//! are resolved even when the process is not root, so a typo is reported from a
//! systemd `User=` unit too; only the switch itself needs root.
//!
//! The feature is not in `default` and `lib.rs` rejects it off unix, so a Windows
//! build never compiles this module (doc/PLAN.md §21).

use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::ptr;

use crate::config::RunAs;
use crate::error::{Error, Result};

/// The largest passwd/group buffer the lookups grow to before giving up.
const MAX_LOOKUP_BUF: usize = 64 * 1024;

/// The ids the process switched to, so `main` can log them once tracing is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dropped {
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
}

/// Resolve `runas` and, when started as root, become that account.
///
/// `Ok(None)` means there was nothing to do: no `runas`, or not root.
pub fn apply(runas: Option<&RunAs>) -> Result<Option<Dropped>> {
    let Some(runas) = runas else {
        return Ok(None);
    };
    let (user, uid, gid) = resolve(runas)?;

    // Not root: nothing to drop (the lookups above still validate the names).
    if unsafe { libc::geteuid() } != 0 {
        return Ok(None);
    }

    // The group before the user: `setgid` and `setgroups` (inside `initgroups`)
    // need the root privilege that `setuid` is about to throw away, and
    // `initgroups` replaces the supplementary groups so none of root's survive.
    if unsafe { libc::setgid(gid) } != 0 {
        let errno = std::io::Error::last_os_error();
        return Err(Error::RunAs(format!("setgid({gid}): {errno}")));
    }
    if unsafe { libc::initgroups(user.as_ptr(), gid) } != 0 {
        let errno = std::io::Error::last_os_error();
        return Err(Error::RunAs(format!("initgroups({}): {errno}", runas.user)));
    }
    if unsafe { libc::setuid(uid) } != 0 {
        let errno = std::io::Error::last_os_error();
        return Err(Error::RunAs(format!("setuid({uid}): {errno}")));
    }

    Ok(Some(Dropped { uid, gid }))
}

/// The lookups, split out so the tests can exercise them without touching the
/// process's own ids. Returns the user's C string for `initgroups`.
fn resolve(runas: &RunAs) -> Result<(CString, libc::uid_t, libc::gid_t)> {
    let user = cstring("user", &runas.user)?;
    let (uid, primary_gid) = lookup_user(&user)?;
    let gid = match &runas.group {
        Some(group) => lookup_group(&cstring("group", group)?)?,
        None => primary_gid,
    };
    Ok((user, uid, gid))
}

fn cstring(field: &str, value: &str) -> Result<CString> {
    CString::new(value).map_err(|_| Error::RunAs(format!("runas.{field} contains a NUL byte")))
}

/// `getpwnam_r`: the uid and the primary gid from the passwd database.
fn lookup_user(user: &CStr) -> Result<(libc::uid_t, libc::gid_t)> {
    let mut len = 1024;
    loop {
        let mut buf = vec![0u8; len];
        let mut passwd = MaybeUninit::<libc::passwd>::uninit();
        let mut result = ptr::null_mut();
        let rc = unsafe {
            libc::getpwnam_r(
                user.as_ptr(),
                passwd.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 {
            if result.is_null() {
                return Err(Error::RunAs(format!(
                    "unknown user: {}",
                    user.to_string_lossy()
                )));
            }
            // Only the two ids are copied out: the rest of the struct points into
            // `buf`, which dies with this iteration.
            let passwd = unsafe { passwd.assume_init() };
            return Ok((passwd.pw_uid, passwd.pw_gid));
        }
        if rc == libc::ERANGE && len < MAX_LOOKUP_BUF {
            len *= 4;
            continue;
        }
        let errno = std::io::Error::from_raw_os_error(rc);
        return Err(Error::RunAs(format!(
            "getpwnam({}): {errno}",
            user.to_string_lossy()
        )));
    }
}

/// `getgrnam_r`: a group's gid from the group database.
fn lookup_group(name: &CStr) -> Result<libc::gid_t> {
    let mut len = 1024;
    loop {
        let mut buf = vec![0u8; len];
        let mut group = MaybeUninit::<libc::group>::uninit();
        let mut result = ptr::null_mut();
        let rc = unsafe {
            libc::getgrnam_r(
                name.as_ptr(),
                group.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 {
            if result.is_null() {
                return Err(Error::RunAs(format!(
                    "unknown group: {}",
                    name.to_string_lossy()
                )));
            }
            let group = unsafe { group.assume_init() };
            return Ok(group.gr_gid);
        }
        if rc == libc::ERANGE && len < MAX_LOOKUP_BUF {
            len *= 4;
            continue;
        }
        let errno = std::io::Error::from_raw_os_error(rc);
        return Err(Error::RunAs(format!(
            "getgrnam({}): {errno}",
            name.to_string_lossy()
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runas(user: &str, group: Option<&str>) -> RunAs {
        RunAs {
            user: user.to_string(),
            group: group.map(str::to_string),
        }
    }

    #[test]
    fn root_resolves_to_uid_zero_and_its_primary_group() {
        let (_, uid, gid) = resolve(&runas("root", None)).expect("root resolves");
        assert_eq!(uid, 0);
        assert_eq!(gid, lookup_user(c"root").expect("root resolves").1);
    }

    #[test]
    fn an_unknown_user_is_reported() {
        let err = resolve(&runas("noobscenic-no-such-user", None)).expect_err("no such user");
        assert!(err.to_string().contains("unknown user"), "{err}");
    }

    #[test]
    fn an_unknown_group_is_reported() {
        let err =
            resolve(&runas("root", Some("noobscenic-no-such-group"))).expect_err("no such group");
        assert!(err.to_string().contains("unknown group"), "{err}");
    }

    #[test]
    fn nothing_to_do_without_runas() {
        assert_eq!(apply(None).expect("no runas is not an error"), None);
    }

    #[test]
    fn a_non_root_process_is_left_alone() {
        if unsafe { libc::geteuid() } == 0 {
            // Never drop the test runner's own privileges; the drop itself is
            // exercised on the deployment target.
            return;
        }
        let dropped = apply(Some(&runas("root", None))).expect("root resolves");
        assert_eq!(dropped, None);
    }
}
