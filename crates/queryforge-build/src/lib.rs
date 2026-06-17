use std::path::PathBuf;

pub struct GenerateBuilder {
    inner: queryforge::build::BuildGenerate,
}

impl GenerateBuilder {
    pub fn new() -> Self {
        Self {
            inner: queryforge::build::BuildGenerate::new(),
        }
    }

    pub fn config(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner = self.inner.config(path);
        self
    }

    pub fn out_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner = self.inner.out_dir(path);
        self
    }

    pub fn watch(mut self, path: impl Into<PathBuf>) -> Self {
        self.inner = self.inner.watch(path);
        self
    }

    pub fn run(self) -> queryforge::Result<queryforge::GenerateReport> {
        self.inner.run()
    }
}

impl Default for GenerateBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn generate() -> GenerateBuilder {
    GenerateBuilder::new()
}
