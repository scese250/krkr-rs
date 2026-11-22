//! Fixture for the `stateless` guard: a file service that keeps its state in
//! itself.  The method that mutates the service must be caught.

pub struct FileSystem {
    root: String,
}

impl FileSystem {
    pub fn host(root: &str) -> Self {
        FileSystem {
            root: root.to_string(),
        }
    }

    pub fn set_root(&mut self, root: &str) {
        self.root = root.to_string();
    }
}
