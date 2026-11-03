//! How keys, sizes and times read on screen, and the folder arithmetic of S3 prefixes.

use std::time::SystemTime;

/// `bytes` in binary units, one decimal from KiB up: what file managers show.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[expect(clippy::cast_precision_loss, reason = "one decimal shown, far coarser than the loss")]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `time` in the local time zone, to the minute.
pub fn when(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(time).format("%Y-%m-%d %H:%M").to_string()
}

/// The name a row shows for `key` inside folder `prefix`: what follows the prefix, a folder keeping its `/`.
pub fn name_in<'a>(key: &'a str, prefix: &str) -> &'a str {
    key.strip_prefix(prefix).filter(|n| !n.is_empty()).unwrap_or(key)
}

/// The last segment of `key`, a folder's without its `/`: a download's suggested file name.
pub fn base_name(key: &str) -> &str {
    let trimmed = key.trim_end_matches('/');
    trimmed.rsplit_once('/').map_or(trimmed, |(_, name)| name)
}

/// The folder holding `prefix` (a folder's prefix ending in `/`, or the top's empty one).
pub fn parent(prefix: &str) -> &str {
    let trimmed = prefix.trim_end_matches('/');
    trimmed.rfind('/').map_or("", |i| &prefix[..=i])
}

/// The path from the bucket's top to `prefix`, each folder with the prefix it opens.
pub fn breadcrumbs(prefix: &str) -> Vec<(&str, &str)> {
    prefix
        .match_indices('/')
        .scan(0, |start, (end, _)| {
            let crumb = (&prefix[*start..end], &prefix[..=end]);
            *start = end + 1;
            Some(crumb)
        })
        .collect()
}

/// The key for `name` typed inside folder `prefix`. Leading slashes are dropped so a name can't escape to the
/// bucket's top by accident; slashes inside make subfolders, as in every S3 tool.
pub fn key_in(prefix: &str, name: &str) -> String {
    format!("{prefix}{}", name.trim().trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.5 KiB");
        assert_eq!(size(5 * 1024 * 1024 * 1024), "5.0 GiB");
    }

    #[test]
    fn names_and_folders_of_keys() {
        assert_eq!(name_in("a/b/c.txt", "a/b/"), "c.txt");
        assert_eq!(name_in("a/b/", "a/"), "b/");
        assert_eq!(base_name("a/b/c.txt"), "c.txt");
        assert_eq!(base_name("a/b/"), "b");
        assert_eq!(parent("a/b/"), "a/");
        assert_eq!(parent("a/"), "");
        assert_eq!(parent(""), "");
        assert_eq!(breadcrumbs("a/b/"), [("a", "a/"), ("b", "a/b/")]);
        assert_eq!(breadcrumbs(""), []);
        assert_eq!(key_in("a/", " /new/x.txt "), "a/new/x.txt");
    }
}
