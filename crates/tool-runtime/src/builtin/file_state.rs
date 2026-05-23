use std::{
    collections::HashMap,
    hash::Hasher,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::SystemTime,
};

/// `WorkspaceReadState` 记录模型已经完整观察过哪些文件。
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceReadState {
    inner: Arc<Mutex<HashMap<PathBuf, WorkspaceFileSnapshot>>>,
}

impl WorkspaceReadState {
    pub(crate) fn record(&self, path: PathBuf, snapshot: WorkspaceFileSnapshot) {
        self.inner
            .lock()
            .expect("workspace read state lock should not be poisoned")
            .insert(path, snapshot);
    }

    pub(crate) fn snapshot(&self, path: &Path) -> Option<WorkspaceFileSnapshot> {
        self.inner
            .lock()
            .expect("workspace read state lock should not be poisoned")
            .get(path)
            .cloned()
    }
}

/// `WorkspaceFileSnapshot` 保存 read 时的文件指纹，用于写入前 stale 检测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspaceFileSnapshot {
    pub(crate) fingerprint: TextFingerprint,
    pub(crate) modified_at: Option<SystemTime>,
    pub(crate) is_complete: bool,
}

/// `TextFingerprint` 是文本内容的轻量内存指纹。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TextFingerprint {
    hash: u64,
    byte_len: u64,
}

impl TextFingerprint {
    pub(crate) fn from_text(text: &str) -> Self {
        Self::from_bytes(text.as_bytes())
    }

    pub(crate) fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write(bytes);
        Self {
            hash: hasher.finish(),
            byte_len: bytes.len() as u64,
        }
    }
}

/// `TextFingerprintBuilder` 增量计算文本指纹，避免为大文件保留完整内容。
#[derive(Debug, Default)]
pub(crate) struct TextFingerprintBuilder {
    hasher: std::collections::hash_map::DefaultHasher,
    byte_len: u64,
}

impl TextFingerprintBuilder {
    pub(crate) fn update(&mut self, bytes: &[u8]) {
        self.hasher.write(bytes);
        self.byte_len += bytes.len() as u64;
    }

    pub(crate) fn finish(self) -> TextFingerprint {
        TextFingerprint {
            hash: self.hasher.finish(),
            byte_len: self.byte_len,
        }
    }
}

pub(crate) fn text_fingerprint_from_reader(reader: &mut dyn Read) -> io::Result<TextFingerprint> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut byte_len = 0u64;
    let mut buffer = [0u8; 8 * 1024];

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.write(&buffer[..read]);
        byte_len += read as u64;
    }

    Ok(TextFingerprint {
        hash: hasher.finish(),
        byte_len,
    })
}
