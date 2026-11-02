//! Fingerprints of images imgoptim has already optimised, so later runs can skip them.

use std::{
    collections::HashSet,
    env,
    fs::{File, OpenOptions, create_dir_all},
    io::{self, BufRead, BufReader, Write},
    num::NonZeroU64,
    path::PathBuf,
    sync::Mutex,
};

use crate::Format;

/// Changed whenever optimising could give different results, so older entries stop matching.
const CACHE_VERSION: u32 = 1;

type Fingerprint = blake3::Hash;

pub struct Cache {
    path: PathBuf,
    zopfli_iterations: NonZeroU64,
    known: HashSet<Fingerprint>,
    added: Mutex<Vec<Fingerprint>>,
}

/// `$XDG_CACHE_HOME/imgoptim`, or `~/.cache/imgoptim`.
fn directory() -> Option<PathBuf> {
    let base = env::var_os("XDG_CACHE_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::home_dir().map(|home| home.join(".cache")))?;
    Some(base.join("imgoptim"))
}

impl Cache {
    /// Load the fingerprints recorded so far. Unreadable lines are skipped, and a missing file is
    /// an empty cache.
    pub fn open(zopfli_iterations: NonZeroU64) -> io::Result<Self> {
        let path = directory()
            .ok_or_else(|| io::Error::other("no home directory for the cache"))?
            .join("optimised");
        let known = match File::open(&path) {
            Ok(file) => BufReader::new(file)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| Fingerprint::from_hex(line.trim()).ok())
                .collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => HashSet::new(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            zopfli_iterations,
            known,
            added: Mutex::new(Vec::new()),
        })
    }

    /// The fingerprint of an image's contents and every setting that affects optimising it.
    fn fingerprint(&self, format: Format, data: &[u8]) -> Fingerprint {
        let mut hasher = blake3::Hasher::new();
        let settings = match format {
            Format::Png => format!("png zopfli={}", self.zopfli_iterations),
            Format::Jpeg => "jpeg".to_owned(),
            Format::Svg => "svg".to_owned(),
        };
        hasher.update(
            format!(
                "imgoptim {} cache {CACHE_VERSION} {settings}\n",
                env!("CARGO_PKG_VERSION")
            )
            .as_bytes(),
        );
        hasher.update(data);
        hasher.finalize()
    }

    /// Whether this image is one imgoptim produced or couldn't improve on, with these settings.
    pub fn contains(&self, format: Format, data: &[u8]) -> bool {
        self.known.contains(&self.fingerprint(format, data))
    }

    /// Record an image as fully optimised.
    pub fn insert(&self, format: Format, data: &[u8]) {
        let fingerprint = self.fingerprint(format, data);
        if !self.known.contains(&fingerprint) {
            self.added
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(fingerprint);
        }
    }

    /// Append the new fingerprints to the cache file in one write, so runs at the same time
    /// don't interleave lines.
    pub fn save(&self) -> io::Result<()> {
        let added = self
            .added
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if added.is_empty() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            create_dir_all(parent)?;
        }
        let mut lines = String::with_capacity(added.len() * 65);
        for fingerprint in added.iter() {
            lines += &fingerprint.to_hex();
            lines.push('\n');
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?
            .write_all(lines.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_depend_on_settings_and_format() {
        let cache = |iterations| Cache {
            path: PathBuf::new(),
            zopfli_iterations: NonZeroU64::new(iterations).unwrap(),
            known: HashSet::new(),
            added: Mutex::new(Vec::new()),
        };
        let data = b"image";
        assert_ne!(
            cache(15).fingerprint(Format::Png, data),
            cache(30).fingerprint(Format::Png, data)
        );
        assert_eq!(
            cache(15).fingerprint(Format::Jpeg, data),
            cache(30).fingerprint(Format::Jpeg, data)
        );
        assert_ne!(
            cache(15).fingerprint(Format::Jpeg, data),
            cache(15).fingerprint(Format::Svg, data)
        );
    }
}
