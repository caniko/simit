use std::fmt;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use semver::Version;

pub const DEFAULT_PATH: &str = "CHANGELOG.md";
pub const HEADER: &str = "# Changelog\n\nAll notable changes to this project will be documented in this file.\n\nThe format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),\nand this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

impl Date {
    fn from_calendar_date(year: i32, month: u8, day: u8) -> Result<Self> {
        if !(1..=12).contains(&month) {
            bail!("month must be between 1 and 12");
        }
        let max_day = days_in_month(year, month);
        if day == 0 || day > max_day {
            bail!("day must be between 1 and {max_day}");
        }
        Ok(Self { year, month, day })
    }

    fn from_unix_days(days: i64) -> Result<Self> {
        let z = days
            .checked_add(719_468)
            .context("current date is outside supported range")?;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = mp + if mp < 10 { 3 } else { -9 };
        let year = y + i64::from(month <= 2);

        let year = i32::try_from(year).context("current year is outside supported range")?;
        let month = u8::try_from(month).context("current month is outside supported range")?;
        let day = u8::try_from(day).context("current day is outside supported range")?;
        Self::from_calendar_date(year, month, day)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Added,
    Changed,
    Deprecated,
    Removed,
    Fixed,
    Security,
}

impl EntryKind {
    pub fn heading(self) -> &'static str {
        match self {
            Self::Added => "### Added",
            Self::Changed => "### Changed",
            Self::Deprecated => "### Deprecated",
            Self::Removed => "### Removed",
            Self::Fixed => "### Fixed",
            Self::Security => "### Security",
        }
    }

    fn sort_key(self) -> usize {
        match self {
            Self::Added => 0,
            Self::Changed => 1,
            Self::Deprecated => 2,
            Self::Removed => 3,
            Self::Fixed => 4,
            Self::Security => 5,
        }
    }

    fn from_heading(line: &str) -> Option<Self> {
        match line.trim() {
            "### Added" => Some(Self::Added),
            "### Changed" => Some(Self::Changed),
            "### Deprecated" => Some(Self::Deprecated),
            "### Removed" => Some(Self::Removed),
            "### Fixed" => Some(Self::Fixed),
            "### Security" => Some(Self::Security),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Document {
    lines: Vec<String>,
    sections: Vec<Section>,
}

#[derive(Debug, Clone)]
struct Section {
    start: usize,
    end: usize,
    kind: SectionKind,
}

#[derive(Debug, Clone)]
enum SectionKind {
    Unreleased,
    Version(Version),
    Other,
}

pub fn init_file(path: &Path) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists; refusing to overwrite it",
            path.display()
        );
    }

    let content = format!("{HEADER}\n## [Unreleased]\n");
    write_file(path, &content)
}

pub fn add_entry_file(path: &Path, kind: EntryKind, text: &str) -> Result<()> {
    let content = read_file(path)?;
    let updated = add_entry(&content, kind, text)?;
    write_file(path, &updated)
}

pub fn release_file(
    path: &Path,
    version: &Version,
    date: Date,
    repo_url: Option<&str>,
    repo_root: Option<&Path>,
) -> Result<()> {
    let content = read_file(path)?;
    let updated = release_content(&content, version, date, repo_url, path, repo_root)?;
    write_file(path, &updated)
}

pub fn release_content(
    content: &str,
    version: &Version,
    date: Date,
    repo_url: Option<&str>,
    path: &Path,
    repo_root: Option<&Path>,
) -> Result<String> {
    let mut document = parse_document(content)?;
    let unreleased = unreleased_section(&document)?;
    let unreleased_start = unreleased.start;
    let unreleased_end = unreleased.end;

    let latest = latest_released_version(&document)?;
    if let Some(latest) = latest.as_ref() {
        if version <= latest {
            bail!(
                "release version {version} must be greater than the latest changelog release {}",
                latest
            );
        }
    }

    let body = trimmed_body_lines(&document.lines[unreleased_start + 1..unreleased_end]);
    if !section_has_entries(&body) {
        bail!("[Unreleased] is empty; add changelog entries before releasing");
    }

    let next_has_content = unreleased_end < document.lines.len();
    let mut replacement = vec![
        "## [Unreleased]".to_owned(),
        String::new(),
        format!("## [{version}] - {date}"),
        String::new(),
    ];
    replacement.extend(body);
    if next_has_content {
        replacement.push(String::new());
    }
    document
        .lines
        .splice(unreleased_start..unreleased_end, replacement);

    let repo = repo_url
        .map(str::to_owned)
        .or_else(|| extract_repo_url_from_footer(content))
        .or_else(|| repo_root.or_else(|| path.parent()).and_then(git_origin_url));
    if let Some(repo) = repo {
        update_reference_links(&mut document.lines, &repo, version, latest.as_ref());
    }

    Ok(render_lines(&document.lines))
}

pub fn check_file(path: &Path) -> Result<()> {
    let content = read_file(path)?;
    check_content(&content)
}

pub fn check_content(content: &str) -> Result<()> {
    if !content.starts_with(HEADER) {
        bail!("CHANGELOG.md must start with simit's canonical Keep a Changelog header");
    }

    let document = parse_document(content)?;
    let unreleased = unreleased_section(&document)?;
    let unreleased_index = document
        .sections
        .iter()
        .position(|section| section.start == unreleased.start)
        .expect("unreleased is present");
    if document.sections[..unreleased_index]
        .iter()
        .any(|section| matches!(section.kind, SectionKind::Version(_)))
    {
        bail!("[Unreleased] must appear before released version sections");
    }

    let mut previous: Option<&Version> = None;
    for section in &document.sections {
        if let SectionKind::Version(version) = &section.kind {
            let heading = &document.lines[section.start];
            parse_version_heading(heading)?;
            if let Some(previous) = previous {
                if version >= previous {
                    bail!(
                        "release sections must be in descending semver order; found {version} after {previous}"
                    );
                }
            }
            previous = Some(version);
        }
    }

    Ok(())
}

pub fn show_file(path: &Path, requested: Option<&str>) -> Result<String> {
    let content = read_file(path)?;
    show_content(&content, requested)
}

pub fn show_content(content: &str, requested: Option<&str>) -> Result<String> {
    let document = parse_document(content)?;
    let section = match requested {
        None => unreleased_section(&document)?,
        Some("Unreleased" | "unreleased") => unreleased_section(&document)?,
        Some(value) => {
            let version =
                Version::parse(value).with_context(|| format!("parsing version `{value}`"))?;
            document
                .sections
                .iter()
                .find(|section| matches!(&section.kind, SectionKind::Version(found) if *found == version))
                .ok_or_else(|| anyhow!("version {version} not found in CHANGELOG.md"))?
        }
    };

    let body = trimmed_body_lines(&document.lines[section.start + 1..section.end]);
    let mut output = body.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

pub fn add_entry(content: &str, kind: EntryKind, text: &str) -> Result<String> {
    let mut document = parse_document(content)?;
    let unreleased = unreleased_section(&document)?;
    let unreleased_start = unreleased.start;
    let unreleased_end = unreleased.end;
    let mut body = trimmed_body_lines(&document.lines[unreleased_start + 1..unreleased_end]);
    insert_entry(&mut body, kind, text);

    let next_has_content = unreleased_end < document.lines.len();
    let mut replacement = vec!["## [Unreleased]".to_owned()];
    if !body.is_empty() || next_has_content {
        replacement.push(String::new());
    }
    replacement.extend(body);
    if next_has_content {
        replacement.push(String::new());
    }
    document
        .lines
        .splice(unreleased_start..unreleased_end, replacement);

    Ok(render_lines(&document.lines))
}

pub fn today_utc() -> Result<Date> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("current system time is before the Unix epoch")?
        .as_secs();
    let days =
        i64::try_from(seconds / 86_400).context("current date is outside supported range")?;
    Date::from_unix_days(days)
}

pub fn parse_iso_date(input: &str) -> Result<Date> {
    let parts = input.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        bail!("date must use YYYY-MM-DD");
    }

    let year = parts[0]
        .parse::<i32>()
        .with_context(|| format!("parsing year in `{input}`"))?;
    let month = parts[1]
        .parse::<u8>()
        .with_context(|| format!("parsing month in `{input}`"))?;
    let day = parts[2]
        .parse::<u8>()
        .with_context(|| format!("parsing day in `{input}`"))?;
    Date::from_calendar_date(year, month, day)
        .with_context(|| format!("parsing calendar date `{input}`"))
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn read_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

fn parse_document(content: &str) -> Result<Document> {
    let lines = content
        .lines()
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect::<Vec<_>>();
    let footer_start = detect_footer_start(&lines).unwrap_or(lines.len());

    let heading_indexes = lines
        .iter()
        .enumerate()
        .take(footer_start)
        .filter_map(|(index, line)| line.starts_with("## ").then_some(index))
        .collect::<Vec<_>>();

    let mut sections = Vec::new();
    for (position, start) in heading_indexes.iter().enumerate() {
        let end = heading_indexes
            .get(position + 1)
            .copied()
            .unwrap_or(footer_start);
        let kind = parse_section_kind(&lines[*start])?;
        sections.push(Section {
            start: *start,
            end,
            kind,
        });
    }

    Ok(Document { lines, sections })
}

fn parse_section_kind(line: &str) -> Result<SectionKind> {
    if line.trim() == "## [Unreleased]" {
        return Ok(SectionKind::Unreleased);
    }

    if line.starts_with("## [") {
        return Ok(SectionKind::Version(parse_version_heading(line)?));
    }

    Ok(SectionKind::Other)
}

fn parse_version_heading(line: &str) -> Result<Version> {
    let Some(label) = line.strip_prefix("## [") else {
        bail!("unsupported changelog heading `{line}`");
    };
    let Some((version, date)) = label.split_once("] - ") else {
        bail!("version heading `{line}` must use `## [x.y.z] - YYYY-MM-DD`");
    };
    let version = Version::parse(version)
        .with_context(|| format!("parsing changelog version in `{line}`"))?;
    parse_iso_date(date).with_context(|| format!("parsing changelog date in `{line}`"))?;
    Ok(version)
}

fn unreleased_section(document: &Document) -> Result<&Section> {
    let mut matches = document
        .sections
        .iter()
        .filter(|section| matches!(section.kind, SectionKind::Unreleased));
    let Some(first) = matches.next() else {
        bail!("CHANGELOG.md must contain `## [Unreleased]`");
    };
    if matches.next().is_some() {
        bail!("CHANGELOG.md must contain exactly one `## [Unreleased]` section");
    }
    Ok(first)
}

fn latest_released_version(document: &Document) -> Result<Option<Version>> {
    let mut latest = None;
    for section in &document.sections {
        if let SectionKind::Version(version) = &section.kind {
            if latest
                .as_ref()
                .is_some_and(|current: &Version| version <= current)
            {
                continue;
            }
            latest = Some(version.clone());
        }
    }
    Ok(latest)
}

fn trimmed_body_lines(lines: &[String]) -> Vec<String> {
    let mut start = 0;
    while start < lines.len() && lines[start].trim().is_empty() {
        start += 1;
    }

    let mut end = lines.len();
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }

    lines[start..end].to_vec()
}

fn section_has_entries(lines: &[String]) -> bool {
    lines.iter().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with("### ")
    })
}

fn insert_entry(body: &mut Vec<String>, kind: EntryKind, text: &str) {
    let heading = kind.heading().to_owned();
    let bullet = format!("- {text}");

    if let Some(index) = body.iter().position(|line| line.trim() == heading) {
        let next_heading = body[index + 1..]
            .iter()
            .position(|line| line.starts_with("### "))
            .map(|offset| index + 1 + offset)
            .unwrap_or(body.len());
        let mut insert_at = next_heading;
        while insert_at > index + 1 && body[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        if insert_at == index + 1 {
            body.splice(insert_at..insert_at, [String::new(), bullet]);
        } else {
            body.insert(insert_at, bullet);
        }
        return;
    }

    let insert_at = body
        .iter()
        .position(|line| {
            EntryKind::from_heading(line)
                .is_some_and(|existing| existing.sort_key() > kind.sort_key())
        })
        .unwrap_or(body.len());

    let mut start = insert_at;
    while start > 0 && body[start - 1].trim().is_empty() {
        start -= 1;
    }
    let mut end = insert_at;
    while end < body.len() && body[end].trim().is_empty() {
        end += 1;
    }

    let mut block = Vec::new();
    if start > 0 {
        block.push(String::new());
    }
    block.push(heading);
    block.push(String::new());
    block.push(bullet);
    if end < body.len() {
        block.push(String::new());
    }

    body.splice(start..end, block);
}

fn detect_footer_start(lines: &[String]) -> Option<usize> {
    let mut index = lines.len();
    let mut saw_link = false;

    while index > 0 {
        let line = lines[index - 1].trim();
        if line.is_empty() {
            index -= 1;
            continue;
        }
        if is_reference_link(line) {
            saw_link = true;
            index -= 1;
            continue;
        }
        break;
    }

    saw_link.then_some(index)
}

fn is_reference_link(line: &str) -> bool {
    line.starts_with('[') && line.contains("]: ") && !line.starts_with("## [")
}

fn render_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        let mut rendered = lines.join("\n");
        rendered.push('\n');
        rendered
    }
}

fn extract_repo_url_from_footer(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        let Some((_, url)) = line.split_once("]: ") else {
            continue;
        };
        let Some((prefix, _)) = url.split_once("/compare/") else {
            continue;
        };
        if prefix.starts_with("http://") || prefix.starts_with("https://") {
            return Some(prefix.trim_end_matches('/').to_owned());
        }
    }
    None
}

fn git_origin_url(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    normalize_repo_url(raw.trim())
}

fn normalize_repo_url(value: &str) -> Option<String> {
    if let Some(stripped) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    {
        return Some(format!(
            "https://{}",
            stripped.trim_end_matches(".git").trim_end_matches('/')
        ));
    }

    if let Some(rest) = value.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!(
            "https://{}/{}",
            host,
            path.trim_end_matches(".git").trim_start_matches('/')
        ));
    }

    let rest = value.strip_prefix("ssh://git@")?;
    let (host, path) = rest.split_once('/')?;
    Some(format!(
        "https://{}/{}",
        host,
        path.trim_end_matches(".git").trim_start_matches('/')
    ))
}

fn update_reference_links(
    lines: &mut Vec<String>,
    repo_url: &str,
    version: &Version,
    previous: Option<&Version>,
) {
    let footer_start = detect_footer_start(lines).unwrap_or(lines.len());
    let existing = if footer_start < lines.len() {
        lines[footer_start..]
            .iter()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let new_unreleased = format!(
        "[Unreleased]: {}/compare/{}...HEAD",
        repo_url.trim_end_matches('/'),
        version
    );
    let new_version = previous.map(|previous| {
        format!(
            "[{version}]: {}/compare/{}...{}",
            repo_url.trim_end_matches('/'),
            previous,
            version
        )
    });

    let mut footer = vec![new_unreleased];
    if let Some(new_version) = new_version {
        footer.push(new_version);
    }
    footer.extend(existing.into_iter().filter(|line| {
        !line.starts_with("[Unreleased]: ") && !line.starts_with(&format!("[{version}]: "))
    }));

    let has_body = !footer.is_empty();
    let separator = footer_start > 0
        && has_body
        && !lines[..footer_start]
            .last()
            .is_some_and(|line| line.trim().is_empty());
    let replace_start = if footer_start < lines.len() {
        footer_start
    } else {
        lines.len()
    };

    let mut replacement = Vec::new();
    if separator {
        replacement.push(String::new());
    }
    replacement.extend(footer);
    lines.splice(replace_start..lines.len(), replacement);
}
