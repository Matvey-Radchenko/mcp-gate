//! Native DACL operations avoid spawning a shell on every private-file read.
use anyhow::{Result, ensure};
use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, LocalFree},
    Security::{Authorization::*, *},
    Storage::FileSystem::FILE_ALL_ACCESS,
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: every allocation below is returned by a LocalFree-owned Win32 API.
        unsafe {
            LocalFree(self.0);
        }
    }
}
struct User(Vec<usize>);
impl User {
    fn current() -> Result<Self> {
        let mut token = null_mut();
        let mut bytes = 0;
        // SAFETY: scalar current-process handle and valid output pointers. The
        // token is closed after the two queries, including a failed query.
        unsafe {
            ensure!(
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0,
                "Cannot identify current Windows user"
            );
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut bytes);
            if bytes < std::mem::size_of::<TOKEN_USER>() as u32 {
                CloseHandle(token);
                anyhow::bail!("Cannot read Windows user token");
            }
            let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
            let ok = GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            );
            CloseHandle(token);
            ensure!(ok != 0, "Cannot read Windows user token");
            Ok(Self(buffer))
        }
    }
    fn sid(&self) -> PSID {
        // SAFETY: current() filled an aligned TOKEN_USER and its SID in this
        // allocation. Moving the Vec never relocates its backing allocation.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}
fn system_sid() -> Result<LocalAllocation> {
    let text: Vec<u16> = "S-1-5-18".encode_utf16().chain(Some(0)).collect();
    let mut sid = null_mut();
    // SAFETY: nul-terminated static SID text and a valid output pointer.
    let ok = unsafe { ConvertStringSidToSidW(text.as_ptr(), &mut sid) };
    ensure!(ok != 0, "Cannot identify Windows SYSTEM");
    Ok(LocalAllocation(sid))
}
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
pub fn protect(path: &Path) -> Result<()> {
    let user = User::current()?;
    let system = system_sid()?;
    let inheritance = if path.is_dir() {
        SUB_CONTAINERS_AND_OBJECTS_INHERIT
    } else {
        0
    };
    let entries = [
        (user.sid(), TRUSTEE_IS_USER),
        (system.0, TRUSTEE_IS_WELL_KNOWN_GROUP),
    ]
    .map(|(sid, kind)| EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: kind,
            ptstrName: sid.cast(),
        },
    });
    let mut acl = null_mut();
    // SAFETY: both SIDs and both entries remain live throughout ACL construction.
    let status =
        unsafe { SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), null(), &mut acl) };
    ensure!(
        status == 0,
        "Cannot construct private Windows ACL ({status})"
    );
    let _allocation = LocalAllocation(acl.cast());
    let path = wide(path);
    // SAFETY: owned nul-terminated path and valid ACL; owner/group/SACL are unchanged.
    let status = unsafe {
        SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null(),
        )
    };
    ensure!(
        status == 0,
        "Cannot protect private Windows file ({status})"
    );
    Ok(())
}
pub fn validate(path: &Path) -> Result<()> {
    let user = User::current()?;
    let system = system_sid()?;
    let path = wide(path);
    let mut descriptor = null_mut();
    let mut acl = null_mut();
    // SAFETY: owned nul-terminated path and valid output pointers. ACL points into
    // the returned descriptor, retained until the entire validation completes.
    let status = unsafe {
        GetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        )
    };
    ensure!(status == 0, "Cannot read private Windows ACL ({status})");
    let _allocation = LocalAllocation(descriptor);
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor and output buffers are valid; subsequent ACE pointers
    // remain inside this descriptor. GetAce validates each index before use.
    unsafe {
        ensure!(
            GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) != 0
                && control & SE_DACL_PROTECTED != 0
                && !acl.is_null()
                && IsValidAcl(acl) != 0,
            "Private Windows file must have a protected non-null DACL"
        );
        for index in 0..(*acl).AceCount {
            let mut pointer = null_mut();
            ensure!(
                GetAce(acl, u32::from(index), &mut pointer) != 0,
                "Cannot inspect Windows ACE"
            );
            let header = &*pointer.cast::<ACE_HEADER>();
            // Basic deny entries cannot grant access. Object/callback ACEs are
            // deliberately rejected until their complete contract is supported.
            if header.AceType == 1 {
                continue;
            }
            ensure!(
                header.AceType == 0
                    && usize::from(header.AceSize) >= std::mem::size_of::<ACCESS_ALLOWED_ACE>(),
                "Unsupported private-file ACE"
            );
            let ace = &*pointer.cast::<ACCESS_ALLOWED_ACE>();
            let sid = std::ptr::addr_of!(ace.SidStart).cast_mut().cast();
            ensure!(
                IsValidSid(sid) != 0
                    && (EqualSid(sid, user.sid()) != 0 || EqualSid(sid, system.0) != 0),
                "Private Windows file grants access to another identity"
            );
        }
    }
    Ok(())
}
