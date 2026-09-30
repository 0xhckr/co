//! Credential files use a protected DACL granting only the current Windows user.
//! Never take ownership of somebody else's config, even from an elevated process.
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    DACL_SECURITY_INFORMATION, EqualSid, GetSecurityDescriptorDacl, GetTokenInformation,
    OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_ALWAYS, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: these buffers are allocated by Win32 conversion/security APIs.
        unsafe {
            LocalFree(self.0);
        }
    }
}

struct Security {
    // usize keeps TOKEN_USER and its inline SID properly aligned.
    user: Vec<usize>,
    descriptor: LocalAllocation,
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
    if value.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "config path contains NUL",
        ));
    }
    value.push(0);
    Ok(value)
}

impl Security {
    fn new() -> io::Result<Self> {
        // SAFETY: every output has a valid writable buffer; handles and local
        // allocations are closed/freed on success and all failure paths.
        unsafe {
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut size = 0;
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
            let mut user = vec![0_usize; (size as usize).div_ceil(size_of::<usize>())];
            let result =
                GetTokenInformation(token, TokenUser, user.as_mut_ptr().cast(), size, &mut size);
            let error = io::Error::last_os_error();
            CloseHandle(token);
            if result == 0 {
                return Err(error);
            }
            let sid = (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            let mut text = null_mut();
            if ConvertSidToStringSidW(sid, &mut text) == 0 {
                return Err(io::Error::last_os_error());
            }
            let allocation = LocalAllocation(text.cast());
            let mut length = 0;
            while *text.add(length) != 0 {
                length += 1;
            }
            let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
            drop(allocation);
            let sddl: Vec<_> = format!("O:{sid_text}D:P(A;OICI;FA;;;{sid_text})")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor = null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                user,
                descriptor: LocalAllocation(descriptor),
            })
        }
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0,
            bInheritHandle: 0,
        }
    }

    fn secure(&self, file: &File) -> io::Result<()> {
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "config cannot be a reparse point",
            ));
        }
        // SAFETY: the file handle and descriptor remain valid for these calls.
        // GetSecurityInfo owns the returned descriptor and all embedded pointers.
        unsafe {
            let handle = file.as_raw_handle();
            let mut owner = null_mut();
            let mut descriptor = null_mut();
            let result = GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            );
            if result != 0 {
                return Err(io::Error::from_raw_os_error(result as i32));
            }
            let allocation = LocalAllocation(descriptor);
            let current = (*self.user.as_ptr().cast::<TOKEN_USER>()).User.Sid;
            if owner.is_null() || EqualSid(owner, current) == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "config is owned by another Windows identity",
                ));
            }
            drop(allocation);
            let mut dacl = null_mut();
            let mut present = 0;
            let mut defaulted = 0;
            if GetSecurityDescriptorDacl(self.descriptor.0, &mut present, &mut dacl, &mut defaulted)
                == 0
                || present == 0
                || dacl.is_null()
            {
                return Err(io::Error::last_os_error());
            }
            let result = SetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            );
            if result != 0 {
                return Err(io::Error::from_raw_os_error(result as i32));
            }
        }
        Ok(())
    }

    fn open(&self, path: &Path, access: u32, disposition: u32) -> io::Result<File> {
        let path = wide(path)?;
        let attributes = self.attributes();
        // SAFETY: null-terminated path and security attributes live through the
        // call. A successful owned handle is transferred exactly once to File.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access | READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                disposition,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(handle) };
        self.secure(&file)?;
        Ok(file)
    }
}

pub(super) fn ensure_directory(path: &Path) -> io::Result<()> {
    let security = Security::new()?;
    if !path.exists() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let name = wide(path)?;
        let attributes = security.attributes();
        // SAFETY: valid path and descriptor, no inheritable handles.
        if unsafe { CreateDirectoryW(name.as_ptr(), &attributes) } == 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
    }
    security.open(path, 0, OPEN_EXISTING)?;
    Ok(())
}

pub(super) fn read(path: &Path) -> io::Result<Vec<u8>> {
    if let Some(parent) = path.parent().filter(|path| path.exists()) {
        ensure_directory(parent)?;
    }
    let mut file = Security::new()?.open(path, GENERIC_READ, OPEN_EXISTING)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(super) fn open_lock(path: &Path) -> io::Result<File> {
    Security::new()?.open(path, GENERIC_READ | GENERIC_WRITE, OPEN_ALWAYS)
}

pub(super) fn create_private(path: &Path) -> io::Result<File> {
    Security::new()?.open(path, GENERIC_WRITE, CREATE_NEW)
}
