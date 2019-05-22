use winapi::um::winnt::*;
use winapi::um::accctrl::*;
use winapi::um::aclapi::*;
use winapi::um::securitybaseapi::*;
use winapi::um::minwinbase::{LPTR, SECURITY_ATTRIBUTES, PSECURITY_ATTRIBUTES};
use winapi::um::winbase::{LocalAlloc, LocalFree};
use winapi::shared::winerror::ERROR_SUCCESS;

use std::ptr;
use std::io;
use std::mem;
use std::marker;

/// Security attributes.
pub struct SecurityAttributes {
    attributes: Option<InnerAttributes>,
}

impl SecurityAttributes {
    /// New default security attributes.
    pub fn empty() -> SecurityAttributes {
        SecurityAttributes { attributes: None }
    }

    /// New default security attributes that allow everyone to connect.
    pub fn allow_everyone_connect() -> io::Result<SecurityAttributes> {
        let attributes = Some(InnerAttributes::allow_everyone(GENERIC_READ | FILE_WRITE_DATA)?);
        Ok(SecurityAttributes { attributes })
    }

    /// New default security attributes that allow everyone to create.
    pub fn allow_everyone_create() -> io::Result<SecurityAttributes> {
        let attributes = Some(InnerAttributes::allow_everyone(GENERIC_READ | GENERIC_WRITE)?);
        Ok(SecurityAttributes { attributes })
    }

    /// Return raw handle of security attributes.
    pub(crate) unsafe fn as_ptr(&mut self) -> PSECURITY_ATTRIBUTES {
        match self.attributes.as_mut() {
            Some(attributes) => attributes.as_ptr(),
            None => ptr::null_mut(),
        }
    }
}

unsafe impl Send for SecurityAttributes {}


struct Sid {
    sid_ptr: PSID
}

impl Sid {
    fn everyone_sid() -> io::Result<Sid> {
        let mut sid_ptr = ptr::null_mut();
        let result = unsafe {
            AllocateAndInitializeSid(
                SECURITY_WORLD_SID_AUTHORITY.as_mut_ptr() as *mut _, 1,
                SECURITY_WORLD_RID,
                0, 0, 0, 0, 0, 0, 0,
                &mut sid_ptr)
        };
        if result == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(Sid{sid_ptr})
        }
    }

    // Unsafe - the returned pointer is only valid for the lifetime of self.
    unsafe fn as_ptr(&self) -> PSID {
        self.sid_ptr
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        if !self.sid_ptr.is_null() {
            unsafe{ FreeSid(self.sid_ptr); }
        }
    }
}


struct AceWithSid<'a> {
    explicit_access: EXPLICIT_ACCESS_W,
    _marker: marker::PhantomData<&'a Sid>,
}

impl<'a> AceWithSid<'a> {
    fn new(sid: &'a Sid, trustee_type: u32) -> AceWithSid<'a> {
        let mut explicit_access = unsafe { mem::zeroed::<EXPLICIT_ACCESS_W>() };
        explicit_access.Trustee.TrusteeForm  = TRUSTEE_IS_SID;
        explicit_access.Trustee.TrusteeType  = trustee_type;
        explicit_access.Trustee.ptstrName    = unsafe { sid.as_ptr() as *mut _ };

        AceWithSid{
            explicit_access,
            _marker: marker::PhantomData,
        }
    }

    fn set_access_mode(&mut self, access_mode: u32) -> &mut Self {
        self.explicit_access.grfAccessMode = access_mode;
        self
    }

    fn set_access_permissions(&mut self, access_permissions: u32) -> &mut Self {
        self.explicit_access.grfAccessPermissions = access_permissions;
        self
    }

    fn allow_inheritance(&mut self, inheritance_flags: u32) -> &mut Self {
        self.explicit_access.grfInheritance = inheritance_flags;
        self
    }
}

struct Acl {
    acl_ptr: PACL,
}

impl Acl {
    fn empty() -> io::Result<Acl> {
        Self::new(&mut [])
    }

    fn new(entries: &mut [AceWithSid]) -> io::Result<Acl> {
        let mut acl_ptr = ptr::null_mut();
        let result = unsafe {
            SetEntriesInAclW(entries.len() as u32,
                entries.as_mut_ptr() as *mut _,
                ptr::null_mut(), &mut acl_ptr)
        };

        if result != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(result as i32));
        }

        Ok(Acl{acl_ptr})
    }

    unsafe fn as_ptr(&self) -> PACL {
        self.acl_ptr
    }
}

impl Drop for Acl {
    fn drop(&mut self) {
        if !self.acl_ptr.is_null() {
            unsafe { LocalFree(self.acl_ptr as *mut _) };
        }
    }
}

struct SecurityDescriptor {
    descriptor_ptr: PSECURITY_DESCRIPTOR,
}

impl SecurityDescriptor{
    fn new() -> io::Result<Self> {
        let descriptor_ptr = unsafe {
            LocalAlloc(LPTR, SECURITY_DESCRIPTOR_MIN_LENGTH)
        };
        if descriptor_ptr.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other,
                                      "Failed to allocate security descriptor"));
        }

        if unsafe { InitializeSecurityDescriptor(
                descriptor_ptr,
                SECURITY_DESCRIPTOR_REVISION) == 0 }
        {
            return Err(io::Error::last_os_error());
        };

        Ok(SecurityDescriptor{descriptor_ptr})
    }

    fn set_dacl(&mut self, acl: &Acl) -> io::Result<()> {
        if unsafe {
            SetSecurityDescriptorDacl(
                self.descriptor_ptr,
                true as i32, acl.as_ptr(),
                false as i32) == 0
        }{
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    unsafe fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.descriptor_ptr
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.descriptor_ptr.is_null() {
            unsafe { LocalFree(self.descriptor_ptr) };
            self.descriptor_ptr = ptr::null_mut();
        }
    }
}

struct InnerAttributes {
    descriptor: SecurityDescriptor,
    acl: Acl,
    attrs: SECURITY_ATTRIBUTES,
}


impl InnerAttributes {

    fn empty() -> io::Result<InnerAttributes> {
        let descriptor = SecurityDescriptor::new()?;
        let mut attrs = unsafe { mem::zeroed::<SECURITY_ATTRIBUTES>() };
        attrs.nLength = mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
        attrs.lpSecurityDescriptor = unsafe {descriptor.as_ptr()};
        attrs.bInheritHandle = false as i32;

        let acl = Acl::empty().expect("this should never fail");

        Ok(InnerAttributes{
            acl,
            descriptor,
            attrs,
        })
    }

    fn allow_everyone(permissions: u32) -> io::Result<InnerAttributes> {
        let mut attributes = Self::empty()?;
        let sid = Sid::everyone_sid()?;
        println!("pisec");

        let mut everyone_ace = AceWithSid::new(&sid, TRUSTEE_IS_WELL_KNOWN_GROUP);
        everyone_ace.set_access_mode(SET_ACCESS)
                    .set_access_permissions(permissions)
                    .allow_inheritance(false as u32);


        let mut entries = vec![everyone_ace];
        attributes.acl = Acl::new(&mut entries)?;
        attributes.descriptor.set_dacl(&attributes.acl)?;

        Ok(attributes)
    }

    unsafe fn as_ptr(&mut self) -> PSECURITY_ATTRIBUTES {
        &mut self.attrs as *mut _
    }
}

#[cfg(test)]
mod test {
    use super::SecurityAttributes;

    #[test]
    fn test_allow_everyone_everything() {
        SecurityAttributes::allow_everyone_create()
            .expect("failed to create security attributes that allow everyone to create a pipe");
    }

    #[test]
    fn test_allow_eveyone_read_write() {
        SecurityAttributes::allow_everyone_connect()
            .expect("failed to create security attributes that allow everyone to read and write to/from a pipe");
    }

}
