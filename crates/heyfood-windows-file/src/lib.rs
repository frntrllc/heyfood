//! Narrow audited Windows boundary for creating and publishing owner-only files.
//!
//! The product crates forbid unsafe code. Windows does not expose security
//! attributes or handle-relative rename through `std`, so those two operations
//! live here behind a safe, ownership-preserving API.

#![deny(unsafe_code)]

#[cfg(windows)]
mod windows {
    use std::ffi::OsStr;
    use std::fs::{self, File};
    use std::io::{self, Write};
    use std::mem::size_of;
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
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_RENAME_INFO, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileAttributeTagInfo, FileRenameInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx, OPEN_EXISTING, READ_CONTROL, SetFileInformationByHandle,
        WRITE_DAC,
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

    /// Stable identity and hard-link count for an already-open Windows file.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct FileIdentity {
        pub volume_serial_number: u32,
        pub file_index: u64,
        pub number_of_links: u32,
    }

    /// Pins one direct directory identity while it is atomically renamed.
    ///
    /// The handle is opened with delete access and remains live through
    /// `SetFileInformationByHandle`, so replacement of the source pathname
    /// cannot redirect the commit to a different directory.
    pub struct DirectoryRenameHandle {
        file: File,
        identity: FileIdentity,
    }

    impl DirectoryRenameHandle {
        /// Open `path` without following its final reparse point.
        #[allow(unsafe_code)]
        pub fn open(path: &Path) -> io::Result<Self> {
            let wide_path = nul_terminated_wide(path.as_os_str())?;
            // SAFETY: `wide_path` is NUL-terminated and alive for the call.
            // DELETE is required by SetFileInformationByHandle; all sharing
            // modes keep ordinary readers and an adversarial path rename from
            // invalidating this identity-pinned handle.
            let handle = unsafe {
                CreateFileW(
                    wide_path.as_ptr(),
                    DELETE | FILE_READ_ATTRIBUTES,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                    ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `handle` is a unique valid owned handle returned by
            // CreateFileW and ownership transfers exactly once to `File`.
            let file = unsafe { File::from_raw_handle(handle) };
            verify_directory(&file)?;
            let identity = file_identity(&file)?;
            Ok(Self { file, identity })
        }

        pub fn identity(&self) -> FileIdentity {
            self.identity
        }

        /// Publish this exact open directory identity at `target`.
        pub fn publish(self, target: &Path, overwrite: bool) -> io::Result<PublishedDirectory> {
            rename_open_file(&self.file, target, overwrite)?;
            verify_directory(&self.file)?;
            Ok(PublishedDirectory { file: self.file })
        }
    }

    /// Keeps the committed directory identity pinned during final inspection.
    pub struct PublishedDirectory {
        file: File,
    }

    impl PublishedDirectory {
        pub fn identity(&self) -> io::Result<FileIdentity> {
            verify_directory(&self.file)?;
            file_identity(&self.file)
        }
    }

    /// Inspect an already-open file without relying on Rust's unstable Windows
    /// by-handle metadata extensions.
    #[allow(unsafe_code)]
    pub fn file_identity(file: &File) -> io::Result<FileIdentity> {
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `information` is a live output buffer of the exact structure
        // expected by Win32 and `file` owns a valid handle for the call.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(FileIdentity {
            volume_serial_number: information.dwVolumeSerialNumber,
            file_index: (u64::from(information.nFileIndexHigh) << 32)
                | u64::from(information.nFileIndexLow),
            number_of_links: information.nNumberOfLinks,
        })
    }

    /// Open an existing directory without following its final reparse point
    /// and return the identity of that exact path entry.
    #[allow(unsafe_code)]
    pub fn open_directory_identity(path: &Path) -> io::Result<FileIdentity> {
        let wide_path = nul_terminated_wide(path.as_os_str())?;
        // SAFETY: `wide_path` is NUL-terminated and alive for the call.
        // BACKUP_SEMANTICS is required to open directories; OPEN_REPARSE_POINT
        // ensures a final reparse point is inspected rather than followed.
        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `handle` is a unique valid owned handle returned by
        // CreateFileW and ownership transfers exactly once to `File`.
        let file = unsafe { File::from_raw_handle(handle) };
        verify_directory(&file)?;
        file_identity(&file)
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
        let wide_path = nul_terminated_wide(path.as_os_str())?;
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
                wide_path.as_ptr(),
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
        if let Err(error) = verify_regular_file(&file) {
            drop(file);
            fs::remove_file(path)?;
            return Err(error);
        }
        Ok(file)
    }

    #[allow(unsafe_code)]
    fn rename_open_file(file: &File, target: &Path, overwrite: bool) -> io::Result<()> {
        let target = wide_without_nul(target.as_os_str())?;
        let (mut buffer, buffer_bytes) = rename_info_buffer(&target, overwrite)?;
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
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
    fn rename_info_buffer(target: &[u16], overwrite: bool) -> io::Result<(Vec<usize>, u32)> {
        let name_bytes = target.len().checked_mul(size_of::<u16>()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "target path is too long")
        })?;
        // Win32 requires at least sizeof(FILE_RENAME_INFO) + FileNameLength.
        // FileNameLength excludes the trailing UTF-16 NUL, which still has to
        // be present in the supplied buffer.
        let buffer_bytes = size_of::<FILE_RENAME_INFO>()
            .checked_add(name_bytes)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "target path is too long")
            })?;
        let word_count = buffer_bytes.div_ceil(size_of::<usize>());
        let mut buffer = vec![0usize; word_count];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: `buffer` is pointer-aligned and large enough for the fixed
        // structure, every UTF-16 code unit copied into the flexible tail, and
        // its zero-initialized trailing NUL.
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
            *ptr::addr_of_mut!((*info).FileName)
                .cast::<u16>()
                .add(target.len()) = 0;
        }
        let buffer_bytes = u32::try_from(buffer_bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path is too long"))?;
        Ok((buffer, buffer_bytes))
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

    #[allow(unsafe_code)]
    fn verify_directory(file: &File) -> io::Result<()> {
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
        if attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "owner-only directory identity is not a direct directory",
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::time::{SystemTime, UNIX_EPOCH};

        #[test]
        #[allow(unsafe_code)]
        fn rename_buffer_includes_nul_when_old_allocation_ended_at_name_boundary() {
            let name_offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
            let mut target = vec![u16::from(b'x')];
            while !(name_offset + target.len() * size_of::<u16>())
                .is_multiple_of(size_of::<usize>())
            {
                target.push(u16::from(b'x'));
            }
            let old_buffer_bytes = name_offset + target.len() * size_of::<u16>();
            assert_eq!(old_buffer_bytes % size_of::<usize>(), 0);

            let (mut buffer, declared_bytes) = rename_info_buffer(&target, false).unwrap();
            let expected_bytes = size_of::<FILE_RENAME_INFO>() + target.len() * size_of::<u16>();
            assert_eq!(usize::try_from(declared_bytes).unwrap(), expected_bytes);
            assert!(buffer.len() * size_of::<usize>() >= expected_bytes);

            let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
            // SAFETY: the production helper guarantees the flexible filename
            // tail and its terminator are within the returned allocation.
            let file_name = unsafe { ptr::addr_of!((*info).FileName).cast::<u16>() };
            for (index, expected) in target.iter().enumerate() {
                // SAFETY: `index` is within the copied filename tail.
                assert_eq!(unsafe { *file_name.add(index) }, *expected);
            }
            // SAFETY: the helper reserves and initializes this extra code unit.
            assert_eq!(unsafe { *file_name.add(target.len()) }, 0);
        }

        #[test]
        fn directory_rename_handle_pins_identity_through_path_substitution() {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "heyfood-windows-directory-commit-{}-{unique}",
                std::process::id()
            ));
            let stage = root.join("stage");
            let displaced = root.join("displaced");
            let target = root.join("published");
            fs::create_dir_all(&stage).unwrap();
            fs::write(stage.join("pinned"), b"expected").unwrap();

            let handle = DirectoryRenameHandle::open(&stage).unwrap();
            let expected_identity = handle.identity();
            fs::rename(&stage, &displaced).unwrap();
            fs::create_dir(&stage).unwrap();
            fs::write(stage.join("replacement"), b"wrong").unwrap();

            let published = handle.publish(&target, false).unwrap();
            assert_eq!(published.identity().unwrap(), expected_identity);
            assert_eq!(fs::read(target.join("pinned")).unwrap(), b"expected");
            assert_eq!(fs::read(stage.join("replacement")).unwrap(), b"wrong");
            drop(published);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[cfg(windows)]
pub use windows::{
    AtomicOwnerOnlyFile, DirectoryRenameHandle, FileIdentity, PublishedDirectory,
    PublishedOwnerOnlyFile, file_identity, open_directory_identity,
};
