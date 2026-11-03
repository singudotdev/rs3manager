//! The AWS CLI's [shared config and credentials files](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html)
//! as their lines, so editing one key of one section leaves every other byte as it was: comments, order, blank lines
//! and the settings r3manager doesn't know (sso, `credential_process`, `services` sections and their nested values).

/// A file's lines, and where each section starts. Lines keep their text exactly; only a whole file's trailing newline
/// is normalized when written.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Document {
    /// The file's lines without their line ends.
    lines: Vec<String>,
}

/// A line as the format reads it.
enum Line<'a> {
    /// `[name]`, its inner text trimmed.
    Header(&'a str),
    /// `key = value` at the start of the line, both trimmed.
    Property(&'a str, &'a str),
    /// A comment, a blank line, or an indented line continuing the property above (a nested sub-setting).
    Other,
}

/// What `line` is. A header's or property's comment after `#` or `;` is only one when preceded by whitespace, as the
/// SDKs read it, so `#` inside a value (a secret key) stays part of it.
fn classify(line: &str) -> Line<'_> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
        return Line::Other;
    }
    if line.starts_with([' ', '\t']) {
        return Line::Other;
    }
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some(end) = rest.find(']')
    {
        return Line::Header(rest[..end].trim());
    }
    match trimmed.split_once('=') {
        Some((key, value)) => Line::Property(key.trim(), strip_comment(value.trim())),
        None => Line::Other,
    }
}

/// `value` without a trailing ` # comment` or ` ; comment`.
fn strip_comment(value: &str) -> &str {
    value
        .char_indices()
        .find(|&(i, c)| (c == '#' || c == ';') && i > 0 && value[..i].ends_with([' ', '\t']))
        .map_or(value, |(i, _)| value[..i].trim_end())
}

impl Document {
    /// The document for a file's `text`.
    pub fn parse(text: &str) -> Self {
        Self { lines: text.lines().map(str::to_owned).collect() }
    }

    /// The file's text, ending with a newline unless empty.
    pub fn render(&self) -> String {
        let mut text = self.lines.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        text
    }

    /// Every section's header text, in file order, repeated ones once.
    pub fn sections(&self) -> Vec<&str> {
        let mut seen = Vec::new();
        for line in &self.lines {
            if let Line::Header(name) = classify(line)
                && !seen.contains(&name)
            {
                seen.push(name);
            }
        }
        seen
    }

    /// The line ranges of every section whose header text is `header`: a file may repeat a header, and the SDKs merge
    /// them.
    fn ranges(&self, header: &str) -> Vec<std::ops::Range<usize>> {
        let mut ranges = Vec::new();
        let mut open: Option<usize> = None;
        for (i, line) in self.lines.iter().enumerate() {
            if let Line::Header(name) = classify(line) {
                if let Some(start) = open.take() {
                    ranges.push(start..i);
                }
                if name == header {
                    open = Some(i);
                }
            }
        }
        if let Some(start) = open {
            ranges.push(start..self.lines.len());
        }
        ranges
    }

    /// The value of `key` in section `header`: the last one, as later lines override earlier ones.
    pub fn get(&self, header: &str, key: &str) -> Option<&str> {
        self.ranges(header)
            .into_iter()
            .flat_map(|range| self.lines[range].iter())
            .filter_map(|line| match classify(line) {
                Line::Property(k, v) if k == key => Some(v),
                _ => None,
            })
            .next_back()
    }

    /// Every key set in section `header`.
    pub fn keys(&self, header: &str) -> Vec<&str> {
        self.ranges(header)
            .into_iter()
            .flat_map(|range| self.lines[range].iter())
            .filter_map(|line| match classify(line) {
                Line::Property(k, _) => Some(k),
                _ => None,
            })
            .collect()
    }

    /// Sets `key` in section `header` to `value`, or removes it with `None`. An existing line is replaced where it is
    /// (and any repeats of it dropped); a new one goes after the section's last property, and a missing section is
    /// added at the end.
    pub fn set(&mut self, header: &str, key: &str, value: Option<&str>) {
        let mut found: Vec<usize> = self
            .ranges(header)
            .into_iter()
            .flatten()
            .filter(|&i| matches!(classify(&self.lines[i]), Line::Property(k, _) if k == key))
            .collect();
        let Some(value) = value else {
            for i in found.into_iter().rev() {
                self.remove_property(i);
            }
            return;
        };
        let line = format!("{key} = {value}");
        if let Some(first) = found.first().copied() {
            self.lines[first] = line;
            found.remove(0);
            for i in found.into_iter().rev() {
                self.remove_property(i);
            }
            return;
        }
        if let Some(range) = self.ranges(header).first().cloned() {
            let after = range
                .clone()
                .rev()
                .find(|&i| i == range.start || !self.lines[i].trim().is_empty())
                .map_or(range.start, |i| self.end_of_property(i));
            self.lines.insert(after + 1, line);
        } else {
            if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                self.lines.push(String::new());
            }
            self.lines.push(format!("[{header}]"));
            self.lines.push(line);
        }
    }

    /// The last line of the property at `i`, its indented continuation lines included.
    fn end_of_property(&self, i: usize) -> usize {
        let mut end = i;
        while end + 1 < self.lines.len() && self.lines[end + 1].starts_with([' ', '\t']) {
            end += 1;
        }
        end
    }

    /// Removes the property at `i` and its indented continuation lines.
    fn remove_property(&mut self, i: usize) {
        let end = self.end_of_property(i);
        self.lines.drain(i..=end);
    }

    /// Removes every section whose header text is `header`. A section's range runs to the next header, so the blank
    /// lines after it go too; blank lines left at the end of the file are dropped.
    pub fn remove_section(&mut self, header: &str) {
        for range in self.ranges(header).into_iter().rev() {
            self.lines.drain(range);
        }
        while self.lines.last().is_some_and(|l| l.trim().is_empty()) {
            self.lines.pop();
        }
    }

    /// Renames every section `from` to `to`, keeping its place and contents.
    pub fn rename_section(&mut self, from: &str, to: &str) {
        for range in self.ranges(from) {
            self.lines[range.start] = format!("[{to}]");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "\
# my profiles
[default]
region = eu-west-1

[profile dev]
endpoint_url = http://127.0.0.1:7480 # local
services = rgw
s3 =
  addressing_style = path
region = default

[services rgw]
s3 =
  endpoint_url = http://other
";

    #[test]
    fn untouched_documents_render_byte_for_byte() {
        assert_eq!(Document::parse(CONFIG).render(), CONFIG);
    }

    #[test]
    fn reads_sections_values_and_strips_comments() {
        let doc = Document::parse(CONFIG);
        assert_eq!(doc.sections(), ["default", "profile dev", "services rgw"]);
        assert_eq!(doc.get("profile dev", "endpoint_url"), Some("http://127.0.0.1:7480"));
        assert_eq!(doc.get("profile dev", "region"), Some("default"));
        // A nested `endpoint_url` under `s3 =` belongs to the services section, not to a profile.
        assert_eq!(doc.get("services rgw", "endpoint_url"), None);
        assert_eq!(doc.get("profile prod", "region"), None);
    }

    #[test]
    fn hash_without_whitespace_before_it_is_part_of_a_secret() {
        let doc = Document::parse("[a]\naws_secret_access_key = ab#cd;ef\n");
        assert_eq!(doc.get("a", "aws_secret_access_key"), Some("ab#cd;ef"));
    }

    #[test]
    fn setting_a_key_replaces_it_in_place() {
        let mut doc = Document::parse(CONFIG);
        doc.set("profile dev", "region", Some("us-east-1"));
        assert_eq!(doc.render(), CONFIG.replace("region = default", "region = us-east-1"));
    }

    #[test]
    fn a_new_key_goes_after_the_sections_last_property_and_its_nested_lines() {
        let mut doc = Document::parse("[profile a]\ns3 =\n  addressing_style = path\n\n[profile b]\n");
        doc.set("profile a", "region", Some("default"));
        assert_eq!(doc.render(), "[profile a]\ns3 =\n  addressing_style = path\nregion = default\n\n[profile b]\n");
    }

    #[test]
    fn a_missing_section_is_appended_after_a_blank_line() {
        let mut doc = Document::parse("[default]\nregion = x\n");
        doc.set("profile new", "region", Some("default"));
        assert_eq!(doc.render(), "[default]\nregion = x\n\n[profile new]\nregion = default\n");
    }

    #[test]
    fn removing_keys_and_sections_leaves_the_rest() {
        let mut doc = Document::parse(CONFIG);
        doc.set("profile dev", "s3", None);
        assert!(!doc.render().contains("addressing_style"));
        doc.remove_section("profile dev");
        assert_eq!(doc.sections(), ["default", "services rgw"]);
        assert!(doc.render().starts_with("# my profiles\n[default]\nregion = eu-west-1\n\n[services rgw]"));
    }

    #[test]
    fn repeated_sections_merge_and_later_values_win() {
        let mut doc = Document::parse("[a]\nregion = one\n[b]\n[a]\nregion = two\n");
        assert_eq!(doc.get("a", "region"), Some("two"));
        doc.set("a", "region", Some("three"));
        assert_eq!(doc.render(), "[a]\nregion = three\n[b]\n[a]\n");
    }

    #[test]
    fn renaming_keeps_place_and_contents() {
        let mut doc = Document::parse(CONFIG);
        doc.rename_section("profile dev", "profile staging");
        assert_eq!(doc.render(), CONFIG.replace("[profile dev]", "[profile staging]"));
    }
}
