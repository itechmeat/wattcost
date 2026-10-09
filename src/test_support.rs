use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

pub(crate) struct FakeFs {
    dir: TempDir,
}

impl FakeFs {
    pub(crate) fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temporary directory"),
        }
    }

    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    pub(crate) fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("relative path has a parent"))
            .expect("create dirs");
        fs::write(path, content).expect("write file");
    }

    pub(crate) fn root(&self) -> crate::providers::FsRoot {
        crate::providers::FsRoot::new(self.dir.path())
    }

    pub(crate) fn mkdir(&self, relative: &str) {
        fs::create_dir_all(self.path(relative)).expect("create dirs");
    }
}

/// A database file in its own temporary directory, opened the way production opens it.
pub(crate) struct TempStore {
    pub(crate) dir: TempDir,
}

impl TempStore {
    pub(crate) fn new() -> Self {
        Self {
            dir: TempDir::new().expect("temporary directory"),
        }
    }

    pub(crate) fn path(&self) -> PathBuf {
        self.dir.path().join("wattcost.db")
    }

    pub(crate) fn open(&self) -> crate::store::Store {
        crate::store::Store::open(&self.path()).expect("open the store")
    }
}
