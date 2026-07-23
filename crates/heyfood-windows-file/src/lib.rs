//! Narrow audited Windows boundary for creating and publishing owner-only files.
//!
//! The product crates forbid unsafe code. Windows does not expose security
//! attributes or handle-relative rename through `std`, so those two operations
//! live here behind a safe, ownership-preserving API.

#![deny(unsafe_code)]

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::{self, Write};
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, FileAttributeTagInfo, FileRenameInfo,
        GetFileInformationByHandleEx, READ_CONTROL, SetFileInformationByHandle, WRITE_DAC,
    };

    /// A newly created regular file whose DACL was protected and restricted to
    /// the current owner in the same `CreateFileW` call that made it visible.
    pub struct AtomicOwnerOnlyFile {
        file: File,
    }

    impl AtomicOwnerOnlyFile {
        /// Exclusively create `path` with a protected, single-owner DACL.
        pub fn create(path: &Path, owner_sid: &str) -> io::Result<Self> {
            create_owner_only(path, owner_sid).map(|file| Self { file })
        }

        pub fn sync_all(&self) -> io::Result<()> {
            self.file.sync_all()
        }

        /// Atomically publish the open file by handle. The handle remains open
        /// and denies delete sharing so a path-based ACL verifier observes this
        /// exact file identity before the caller drops the returned guard.
        pub fn publish(self, target: &Path, overwrite: bool) -> io::Result<PublishedOwnerOnlyFile> {
            rename_open_file(&self.file, target, overwrite)?;
            Ok(PublishedOwnerOnlyFile { file: self.file })
        }
    }

    impl Write for AtomicOwnerOnlyFile {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.file.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    /// Keeps the published identity open without delete sharing while the
    /// product layer performs its independent final ACL verification.
    pub struct PublishedOwnerOnlyFile {
        file: File,
    }

    impl PublishedOwnerOnlyFile {
        /// Verify the identity reached through the still-open published handle,
        /// rather than reopening the user-controlled path.
        pub fn verify_regular(&self) -> io::Result<()> {
            verify_regular_file(&self.file)
        }
    }

    struct LocalSecurityDescriptor(PSECURITY_DESCRIPTOR);

    impl Drop for LocalSecurityDescriptor {
        #[allow(unsafe_code)]
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: the descriptor is returned by
                // ConvertStringSecurityDescriptorToSecurityDescriptorW and is
                // owned by this guard exactly once.
                unsafe {
                    let _ = LocalFree(self.0);
                }
            }
        }
    }

    #[allow(unsafe_code)]
    fn create_owner_only(path: &Path, owner_sid: &str) -> io::Result<File> {
        let path = nul_terminated_wide(path.as_os_str())?;
        let sddl = nul_terminated_wide(OsStr::new(&format!(
            "O:{owner_sid}D:P(A;;FA;;;{owner_sid})"
        )))?;
        let mut raw_descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `sddl` is NUL-terminated and alive for the call; the output
        // pointer is initialized to null and then owned by the local guard.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut raw_descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let descriptor = LocalSecurityDescriptor(raw_descriptor);
        let security = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>())
                .expect("SECURITY_ATTRIBUTES size fits in u32"),
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // SAFETY: all pointers reference live, correctly initialized Win32
        // structures. CREATE_NEW prevents opening or following an existing
        // final component, and the protected DACL is installed atomically.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE | DELETE | READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &security,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is a unique valid owned file handle returned by
        // CreateFileW and ownership transfers exactly once to `File`.
        let file = unsafe { File::from_raw_handle(handle) };
        verify_regular_file(&file)?;
        Ok(file)
    }

    #[allow(unsafe_code)]
    fn rename_open_file(file: &File, target: &Path, overwrite: bool) -> io::Result<()> {
        let target = wide_without_nul(target.as_os_str())?;
        let name_bytes = target.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "target path is too long")
        })?;
        let name_offset = offset_of!(FILE_RENAME_INFO, FileName);
        let buffer_bytes = name_offset.checked_add(name_bytes).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "target path is too long")
        })?;
        let word_count = buffer_bytes.div_ceil(size_of::<usize>());
        let mut buffer = vec![0usize; word_count];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: `buffer` is pointer-aligned and large enough for the fixed
        // header plus every UTF-16 code unit copied into the flexible tail.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = overwrite;
            (*info).RootDirectory = ptr::null_mut();
            (*info).FileNameLength = u32::try_from(name_bytes).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "target path is too long")
            })?;
            ptr::copy_nonoverlapping(
                target.as_ptr(),
                ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                target.len(),
            );
        }
        let buffer_bytes = u32::try_from(buffer_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path is too long"))?;
        // SAFETY: `file` remains alive, was opened with DELETE access, and the
        // rename buffer matches FILE_RENAME_INFO for its full declared size.
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileRenameInfo,
                info.cast(),
                buffer_bytes,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            return Err(match error.raw_os_error().map(|value| value as u32) {
                Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => {
                    io::Error::new(io::ErrorKind::AlreadyExists, "target already exists")
                }
                _ => error,
            });
        }
        Ok(())
    }

    #[allow(unsafe_code)]
    fn verify_regular_file(file: &File) -> io::Result<()> {
        let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: `attributes` is a live output buffer of the exact class size
        // and `file` owns a valid handle for the duration of the call.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileAttributeTagInfo,
                ptr::addr_of_mut!(attributes).cast(),
                u32::try_from(size_of::<FILE_ATTRIBUTE_TAG_INFO>())
                    .expect("FILE_ATTRIBUTE_TAG_INFO size fits in u32"),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if attributes.FileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY)
            != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "owner-only file identity is not a regular file",
            ));
        }
        Ok(())
    }

    fn nul_terminated_wide(value: &OsStr) -> io::Result<Vec<u16>> {
        let mut wide = wide_without_nul(value)?;
        wide.push(0);
        Ok(wide)
    }

    fn wide_without_nul(value: &OsStr) -> io::Result<Vec<u16>> {
        let wide = value.encode_wide().collect::<Vec<_>>();
        if wide.is_empty() || wide.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows path is empty or contains NUL",
            ));
        }
        Ok(wide)
    }
}

#[cfg(windows)]
pub use windows::{AtomicOwnerOnlyFile, PublishedOwnerOnlyFile};
