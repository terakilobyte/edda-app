//! Platform-neutral ownership for helper process trees.
//!
//! Voice engines describe *what* to launch; this module decides *how* the
//! whole process tree lives and dies. Windows uses a kill-on-close Job
//! Object. A Linux backend can replace `platform` with a process-group or
//! systemd-scope implementation without touching engine installers or UI.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::process::{Child, Command};
use std::sync::Mutex;

pub struct HelperManager {
    children: Mutex<HashMap<String, ManagedChild>>,
}

struct ManagedChild {
    child: Child,
    _tree: platform::ProcessTree,
}

impl HelperManager {
    pub fn new() -> Self {
        Self { children: Mutex::new(HashMap::new()) }
    }

    /// Ask the OS for an unused loopback port. Launchers should start
    /// immediately and retry with a new port if another process wins the
    /// small bind race.
    pub fn free_loopback_port() -> std::io::Result<u16> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
        Ok(listener.local_addr()?.port())
    }

    pub fn spawn(&self, name: impl Into<String>, command: &mut Command) -> anyhow::Result<u32> {
        let name = name.into();
        self.stop(&name);
        let tree = platform::ProcessTree::new()?;
        let mut child = command.spawn()?;
        if let Err(e) = tree.attach(&child) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        let pid = child.id();
        self.children.lock().unwrap_or_else(|e| e.into_inner()).insert(name, ManagedChild { child, _tree: tree });
        Ok(pid)
    }

    pub fn stop(&self, name: &str) -> bool {
        let old = self.children.lock().unwrap_or_else(|e| e.into_inner()).remove(name);
        if let Some(mut managed) = old {
            let _ = managed.child.kill();
            let _ = managed.child.wait();
            true
        } else {
            false
        }
    }

    pub fn running(&self, name: &str) -> bool {
        let mut children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        let Some(managed) = children.get_mut(name) else { return false };
        matches!(managed.child.try_wait(), Ok(None))
    }
}

impl Drop for HelperManager {
    fn drop(&mut self) {
        let children = self.children.get_mut().unwrap_or_else(|e| e.into_inner());
        for managed in children.values_mut() {
            let _ = managed.child.kill();
        }
        children.clear(); // closes every process-tree owner
    }
}

#[cfg(windows)]
mod platform {
    use anyhow::{bail, Context};
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE};

    pub struct ProcessTree { job: HANDLE }
    unsafe impl Send for ProcessTree {}

    impl ProcessTree {
        pub fn new() -> anyhow::Result<Self> {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() { return Err(std::io::Error::last_os_error()).context("creating helper Job Object") }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe { SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as _, size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) };
            if ok == 0 { unsafe { CloseHandle(job) }; return Err(std::io::Error::last_os_error()).context("configuring helper Job Object") }
            Ok(Self { job })
        }

        pub fn attach(&self, child: &Child) -> anyhow::Result<()> {
            let process = child.as_raw_handle() as HANDLE;
            if unsafe { AssignProcessToJobObject(self.job, process) } == 0 {
                bail!("assigning helper to Job Object: {}", std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for ProcessTree {
        fn drop(&mut self) { unsafe { CloseHandle(self.job) }; }
    }
}

#[cfg(not(windows))]
mod platform {
    use std::process::Child;

    /// Portable fallback owns the immediate child. The future Linux backend
    /// belongs here and will create a process group before spawn.
    pub struct ProcessTree;
    impl ProcessTree {
        pub fn new() -> anyhow::Result<Self> { Ok(Self) }
        pub fn attach(&self, _child: &Child) -> anyhow::Result<()> { Ok(()) }
    }
}

#[cfg(test)]
mod tests {
    use super::HelperManager;

    #[test]
    fn allocated_port_is_loopback_and_available() {
        let port = HelperManager::free_loopback_port().unwrap();
        assert!(port > 0);
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).unwrap();
    }
}
