//! Fixture for the `stateless` guard: the clean near-miss.  The service is
//! built with its state and only reads it afterwards.

pub struct FileSystem {
    root: String,
}

impl FileSystem {
    pub fn host(root: &str) -> Self {
        FileSystem {
            root: root.to_string(),
        }
    }

    pub fn root(&self) -> &str {
        &self.root
    }
}
