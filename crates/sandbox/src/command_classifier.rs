//! Command sanitization and classification for the MCP Bash tool.
//!
//! Provides two layers of defense-in-depth on top of VM isolation:
//!
//! 1. **Sanitization** — hard-rejects inherently dangerous or malformed commands
//!    (fork bombs, privilege escalation, raw device access) before they reach the VM.
//!
//! 2. **Classification** — labels commands as `ReadOnly`, `Write`, or `Destructive`
//!    and returns the label as response metadata. Claude Code handles its own
//!    client-side permission prompts, so this is informational / for logging.
//!
//! Classification lists are configurable via [`CommandClassifierConfig`], which can
//! be loaded from a TOML config file. Sanitization rules are hardcoded and not
//! user-configurable (security-critical).

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Result of command sanitization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SanitizeResult {
    Ok,
    Rejected { reason: String },
}

/// Classification tier for a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandClassification {
    ReadOnly,
    Write,
    Destructive,
}

impl fmt::Display for CommandClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnly => write!(f, "read_only"),
            Self::Write => write!(f, "write"),
            Self::Destructive => write!(f, "destructive"),
        }
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// User-configurable command classification lists.
///
/// Each field holds the list of commands (or subcommands) that belong to a
/// particular classification tier. The `Default` implementation populates
/// every field with the built-in allowlists.
///
/// Serialize/deserialize with `#[serde(default)]` so that missing fields in
/// a TOML file fall back to the defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandClassifierConfig {
    pub read_only_commands: Vec<String>,
    pub write_commands: Vec<String>,
    pub destructive_commands: Vec<String>,
    pub git_read_only_subcommands: Vec<String>,
    pub git_destructive_subcommands: Vec<String>,
    pub cargo_read_only_subcommands: Vec<String>,
    pub cargo_destructive_subcommands: Vec<String>,
    pub npm_read_only_subcommands: Vec<String>,
    pub npm_read_only_scripts: Vec<String>,
}

impl Default for CommandClassifierConfig {
    fn default() -> Self {
        Self {
            read_only_commands: DEFAULT_READ_ONLY_COMMANDS.iter().map(|s| s.to_string()).collect(),
            write_commands: DEFAULT_WRITE_COMMANDS.iter().map(|s| s.to_string()).collect(),
            destructive_commands: DEFAULT_DESTRUCTIVE_COMMANDS.iter().map(|s| s.to_string()).collect(),
            git_read_only_subcommands: DEFAULT_GIT_READ_ONLY_SUBCOMMANDS.iter().map(|s| s.to_string()).collect(),
            git_destructive_subcommands: DEFAULT_GIT_DESTRUCTIVE_SUBCOMMANDS.iter().map(|s| s.to_string()).collect(),
            cargo_read_only_subcommands: DEFAULT_CARGO_READ_ONLY_SUBCOMMANDS.iter().map(|s| s.to_string()).collect(),
            cargo_destructive_subcommands: DEFAULT_CARGO_DESTRUCTIVE_SUBCOMMANDS.iter().map(|s| s.to_string()).collect(),
            npm_read_only_subcommands: DEFAULT_NPM_READ_ONLY_SUBCOMMANDS.iter().map(|s| s.to_string()).collect(),
            npm_read_only_scripts: DEFAULT_NPM_READ_ONLY_SCRIPTS.iter().map(|s| s.to_string()).collect(),
        }
    }
}

const DEFAULT_READ_ONLY_COMMANDS: &[&str] = &[
    "cd", "ls", "cat", "head", "tail", "less", "more", "wc", "file", "find",
    "grep", "egrep", "fgrep", "rg", "ag", "awk", "gawk", "which", "whereis",
    "type", "echo", "printf", "pwd", "env", "printenv", "whoami", "id",
    "hostname", "uname", "date", "cal", "uptime", "df", "du", "free", "top",
    "htop", "ps", "stat", "readlink", "realpath", "basename", "dirname",
    "test", "[", "true", "false", "diff", "cmp", "md5sum", "sha256sum",
    "sha1sum", "sha512sum", "xxd", "od", "strings", "tree", "bat", "jq",
    "yq", "sort", "uniq", "cut", "tr", "column", "comm", "join", "paste",
    "fold", "rev", "tac", "nl", "expand", "unexpand", "hexdump", "man",
    "help", "info",
];

const DEFAULT_WRITE_COMMANDS: &[&str] = &[
    "touch", "mkdir", "cp", "mv", "chmod", "chown", "chgrp", "curl", "wget",
    "tar", "unzip", "zip", "gzip", "gunzip", "bzip2", "bunzip2", "xz",
    "unxz", "make", "cmake", "patch", "ln",
];

const DEFAULT_DESTRUCTIVE_COMMANDS: &[&str] = &[
    "rm", "rmdir", "dd", "mkfs", "shred", "truncate",
];

const DEFAULT_GIT_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "status", "log", "diff", "show", "branch", "tag", "remote", "rev-parse",
    "ls-files", "ls-tree", "describe", "shortlog", "blame", "bisect",
    "reflog", "stash list", "config", "help", "version",
];

const DEFAULT_GIT_DESTRUCTIVE_SUBCOMMANDS: &[&str] = &[
    "clean",
];

const DEFAULT_CARGO_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "check", "test", "clippy", "doc", "bench", "metadata", "tree", "version", "help",
];

const DEFAULT_CARGO_DESTRUCTIVE_SUBCOMMANDS: &[&str] = &[
    "clean",
];

const DEFAULT_NPM_READ_ONLY_SUBCOMMANDS: &[&str] = &[
    "test", "list", "ls", "view", "info", "outdated", "help", "version",
];

const DEFAULT_NPM_READ_ONLY_SCRIPTS: &[&str] = &[
    "test", "lint", "check", "typecheck", "type-check", "validate",
];

// ---------------------------------------------------------------------------
// CommandClassifier struct (pre-computed HashSets for O(1) lookup)
// ---------------------------------------------------------------------------

/// A command classifier that uses pre-computed `HashSet`s for O(1) lookup.
///
/// Constructed from a [`CommandClassifierConfig`] via [`CommandClassifier::new`],
/// or from defaults via [`CommandClassifier::with_defaults`].
pub struct CommandClassifier {
    read_only: HashSet<String>,
    write: HashSet<String>,
    destructive: HashSet<String>,
    git_read_only: HashSet<String>,
    git_destructive: HashSet<String>,
    cargo_read_only: HashSet<String>,
    cargo_destructive: HashSet<String>,
    npm_read_only: HashSet<String>,
    npm_read_only_scripts: HashSet<String>,
}

impl CommandClassifier {
    /// Build a classifier from a user-provided config.
    pub fn new(config: CommandClassifierConfig) -> Self {
        Self {
            read_only: config.read_only_commands.into_iter().collect(),
            write: config.write_commands.into_iter().collect(),
            destructive: config.destructive_commands.into_iter().collect(),
            git_read_only: config.git_read_only_subcommands.into_iter().collect(),
            git_destructive: config.git_destructive_subcommands.into_iter().collect(),
            cargo_read_only: config.cargo_read_only_subcommands.into_iter().collect(),
            cargo_destructive: config.cargo_destructive_subcommands.into_iter().collect(),
            npm_read_only: config.npm_read_only_subcommands.into_iter().collect(),
            npm_read_only_scripts: config.npm_read_only_scripts.into_iter().collect(),
        }
    }

    /// Build a classifier with built-in default lists.
    pub fn with_defaults() -> Self {
        Self::new(CommandClassifierConfig::default())
    }

    /// Classify a command as ReadOnly, Write, or Destructive.
    ///
    /// Splits on shell operators, handles quoting, command substitution, subshells,
    /// and redirects. Returns the maximum classification across all segments.
    pub fn classify(&self, command: &str) -> CommandClassification {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return CommandClassification::ReadOnly;
        }

        let mut max_class = CommandClassification::ReadOnly;

        // Check for output redirects in unquoted context
        max_class = max_class.max(classify_redirects(trimmed));

        // Split into segments and classify each
        for segment in split_command_segments(trimmed) {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }
            let class = self.classify_single_segment(segment);
            max_class = max_class.max(class);
        }

        // Check for command substitution, subshells, process substitution
        max_class = max_class.max(self.classify_nested(trimmed));

        max_class
    }

    /// Classify a single simple command segment (no shell operators).
    fn classify_single_segment(&self, segment: &str) -> CommandClassification {
        let words: Vec<&str> = shell_words(segment);
        if words.is_empty() {
            return CommandClassification::ReadOnly;
        }

        let command_name = extract_command_name(words[0]);
        let args = &words[1..];

        self.classify_command_with_args(command_name, args)
    }

    /// Classify based on command name and its arguments.
    fn classify_command_with_args(&self, command: &str, args: &[&str]) -> CommandClassification {
        // Context-sensitive commands first
        match command {
            "git" => return self.classify_git(args),
            "cargo" => return self.classify_cargo(args),
            "npm" | "npx" => return self.classify_npm(command, args),
            "sed" => return classify_sed(args),
            "python" | "python3" => return classify_python(args),
            "node" => return classify_node(args),
            "tee" => return classify_tee(args),
            "xargs" => return self.classify_xargs(args),
            _ => {}
        }

        if self.read_only.contains(command) {
            return CommandClassification::ReadOnly;
        }

        if self.destructive.contains(command) {
            return CommandClassification::Destructive;
        }

        if self.write.contains(command) {
            return CommandClassification::Write;
        }

        // Unknown commands default to Write
        CommandClassification::Write
    }

    /// Extract and classify nested constructs: $(...), `...`, (...), >(...)
    fn classify_nested(&self, command: &str) -> CommandClassification {
        let mut max_class = CommandClassification::ReadOnly;

        for inner in extract_parenthesized_contents(command) {
            let inner_class = self.classify(&inner);
            max_class = max_class.max(inner_class);
        }

        for inner in extract_backtick_contents(command) {
            let inner_class = self.classify(&inner);
            max_class = max_class.max(inner_class);
        }

        max_class
    }

    fn classify_git(&self, args: &[&str]) -> CommandClassification {
        let subcommand = args.first().copied().unwrap_or("");

        if self.git_read_only.contains(subcommand) {
            return CommandClassification::ReadOnly;
        }

        // Configurable destructive subcommands
        if self.git_destructive.contains(subcommand) {
            return CommandClassification::Destructive;
        }

        // Hardcoded destructive patterns (argument-dependent, not configurable)
        if subcommand == "reset" && args.contains(&"--hard") {
            return CommandClassification::Destructive;
        }
        if subcommand == "push"
            && args
                .iter()
                .any(|a| *a == "--force" || *a == "-f" || a.starts_with("--force-with-lease"))
        {
            return CommandClassification::Destructive;
        }
        if subcommand == "checkout" && args.contains(&".") {
            return CommandClassification::Destructive;
        }

        CommandClassification::Write
    }

    fn classify_cargo(&self, args: &[&str]) -> CommandClassification {
        let subcommand = args.first().copied().unwrap_or("");

        if self.cargo_read_only.contains(subcommand) {
            return CommandClassification::ReadOnly;
        }

        if self.cargo_destructive.contains(subcommand) {
            return CommandClassification::Destructive;
        }

        CommandClassification::Write
    }

    fn classify_npm(&self, command: &str, args: &[&str]) -> CommandClassification {
        let subcommand = args.first().copied().unwrap_or("");

        // npx with tsc/eslint/prettier --check is read-only
        if command == "npx" {
            let tool = args.first().copied().unwrap_or("");
            if tool == "tsc" && args.contains(&"--noEmit") {
                return CommandClassification::ReadOnly;
            }
            return CommandClassification::Write;
        }

        if self.npm_read_only.contains(subcommand) {
            return CommandClassification::ReadOnly;
        }

        // npm run <script> — classify by script name
        if subcommand == "run" || subcommand == "run-script" {
            let script = args.get(1).copied().unwrap_or("");
            if self.npm_read_only_scripts.contains(script) {
                return CommandClassification::ReadOnly;
            }
        }

        if subcommand == "cache" && args.contains(&"clean") {
            return CommandClassification::Destructive;
        }

        CommandClassification::Write
    }

    fn classify_xargs(&self, args: &[&str]) -> CommandClassification {
        let mut i = 0;
        while i < args.len() {
            let arg = args[i];
            if arg == "-I" || arg == "-L" || arg == "-n" || arg == "-P" || arg == "-d" {
                i += 2;
                continue;
            }
            if arg.starts_with('-') {
                i += 1;
                continue;
            }
            return self.classify_command_with_args(extract_command_name(arg), &args[i + 1..]);
        }
        CommandClassification::ReadOnly
    }
}

// ---------------------------------------------------------------------------
// Sanitization
// ---------------------------------------------------------------------------

/// Reject inherently dangerous or malformed commands.
///
/// This is a hard block — rejected commands never reach the VM.
pub fn sanitize(command: &str) -> SanitizeResult {
    let trimmed = command.trim();

    if trimmed.is_empty() {
        return SanitizeResult::Rejected {
            reason: "empty command".to_string(),
        };
    }

    if command.contains('\0') {
        return SanitizeResult::Rejected {
            reason: "command contains null byte".to_string(),
        };
    }

    // Fork bomb patterns
    if is_fork_bomb(trimmed) {
        return SanitizeResult::Rejected {
            reason: "fork bomb detected".to_string(),
        };
    }

    // Privilege escalation — check each segment for leading sudo/su/doas
    if contains_privilege_escalation(trimmed) {
        return SanitizeResult::Rejected {
            reason: "privilege escalation commands are not allowed".to_string(),
        };
    }

    // Raw device access
    if contains_raw_device_access(trimmed) {
        return SanitizeResult::Rejected {
            reason: "raw device access is not allowed".to_string(),
        };
    }

    // Kernel module manipulation
    if contains_kernel_module_commands(trimmed) {
        return SanitizeResult::Rejected {
            reason: "kernel module manipulation is not allowed".to_string(),
        };
    }

    SanitizeResult::Ok
}

/// Detect common fork bomb patterns.
fn is_fork_bomb(command: &str) -> bool {
    // Classic bash fork bomb: :(){ :|:& };:
    // Also match variants with different function names
    let normalized = command.replace(' ', "");
    if normalized.contains("(){:|:&};") {
        return true;
    }

    // While-true fork: while true; do ... & done
    if command.contains("while") && command.contains("true") && command.contains('&') {
        let lower = command.to_lowercase();
        if (lower.contains("while true") || lower.contains("while :"))
            && (lower.contains("& done") || lower.contains("&done"))
        {
            return true;
        }
    }

    false
}

/// Check if the command contains privilege escalation at the start of any segment.
fn contains_privilege_escalation(command: &str) -> bool {
    let escalation_commands = ["sudo", "su", "doas", "pkexec"];
    for segment in split_segments_simple(command) {
        let first_word = first_word_of(segment.trim());
        if escalation_commands.contains(&first_word) {
            return true;
        }
    }
    false
}

/// Check if the command references raw block devices.
fn contains_raw_device_access(command: &str) -> bool {
    let device_patterns = ["/dev/sd", "/dev/nvme", "/dev/loop", "/dev/vd", "/dev/hd"];
    // Scan outside of quotes
    for token in unquoted_tokens(command) {
        for pattern in &device_patterns {
            if token.contains(pattern) {
                return true;
            }
        }
    }
    false
}

/// Check for kernel module manipulation commands.
fn contains_kernel_module_commands(command: &str) -> bool {
    let module_commands = ["insmod", "modprobe", "rmmod", "depmod"];
    for segment in split_segments_simple(command) {
        let first_word = first_word_of(segment.trim());
        if module_commands.contains(&first_word) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Classification
// ---------------------------------------------------------------------------

/// Classify a command using the built-in default lists.
///
/// This is a convenience wrapper around [`CommandClassifier::with_defaults`].
/// For configurable classification, construct a [`CommandClassifier`] from a
/// [`CommandClassifierConfig`] and call [`CommandClassifier::classify`] directly.
pub fn classify(command: &str) -> CommandClassification {
    CommandClassifier::with_defaults().classify(command)
}

/// Classify redirect operators found outside quotes.
fn classify_redirects(command: &str) -> CommandClassification {
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;
    let chars: Vec<char> = command.chars().collect();
    let mut max_class = CommandClassification::ReadOnly;
    let mut i = 0;

    while i < chars.len() {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        let ch = chars[i];

        if ch == '\\' && !in_single_quote {
            escape_next = true;
            i += 1;
            continue;
        }

        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }

        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }

        if in_single_quote || in_double_quote {
            i += 1;
            continue;
        }

        // Detect > and >> outside quotes
        if ch == '>' {
            let prev = if i > 0 { Some(chars[i - 1]) } else { None };
            let next = chars.get(i + 1).copied();

            // Heredoc: << (skip the > that follows <)
            if prev == Some('<') {
                i += 1;
                continue;
            }

            // Process substitution: >( — classified separately via classify_nested
            if next == Some('(') {
                i += 1;
                continue;
            }

            // >> is append (Write), > is overwrite (Destructive)
            if next == Some('>') {
                max_class = max_class.max(CommandClassification::Write);
                i += 2; // skip both '>' characters
                continue;
            } else {
                max_class = max_class.max(CommandClassification::Destructive);
            }
        }

        i += 1;
    }

    max_class
}


/// Extract contents of $(...), >(...), and bare (...) subshells, respecting quotes.
fn extract_parenthesized_contents(command: &str) -> Vec<String> {
    let mut results = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        let ch = chars[i];

        if ch == '\\' && !in_single_quote {
            escape_next = true;
            i += 1;
            continue;
        }

        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }

        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            i += 1;
            continue;
        }

        if in_single_quote || in_double_quote {
            i += 1;
            continue;
        }

        // Match $( or >( or bare ( at start-of-segment
        let is_dollar_paren = ch == '$' && i + 1 < len && chars[i + 1] == '(';
        let is_process_sub = ch == '>' && i + 1 < len && chars[i + 1] == '(';
        let is_input_sub = ch == '<' && i + 1 < len && chars[i + 1] == '(';
        let is_bare_subshell = ch == '(' && (i == 0 || is_segment_boundary(chars[i - 1]));

        if is_dollar_paren || is_process_sub || is_input_sub {
            // Start after the opening paren
            let start = i + 2;
            if let Some((end, inner)) = find_matching_paren(&chars, start) {
                results.push(inner);
                i = end + 1;
                continue;
            }
        } else if is_bare_subshell {
            let start = i + 1;
            if let Some((end, inner)) = find_matching_paren(&chars, start) {
                results.push(inner);
                i = end + 1;
                continue;
            }
        }

        i += 1;
    }

    results
}

/// Find the matching closing paren, handling nesting and quotes.
/// `start` is the index right after the opening `(`.
/// Returns (index_of_closing_paren, inner_content).
fn find_matching_paren(chars: &[char], start: usize) -> Option<(usize, String)> {
    let mut depth = 1;
    let mut i = start;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;
    let mut inner = String::new();

    while i < chars.len() && depth > 0 {
        if escape_next {
            escape_next = false;
            inner.push(chars[i]);
            i += 1;
            continue;
        }

        let ch = chars[i];

        if ch == '\\' && !in_single_quote {
            escape_next = true;
            inner.push(ch);
            i += 1;
            continue;
        }

        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            inner.push(ch);
            i += 1;
            continue;
        }

        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            inner.push(ch);
            i += 1;
            continue;
        }

        if !in_single_quote && !in_double_quote {
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
                if depth == 0 {
                    return Some((i, inner));
                }
            }
        }

        inner.push(ch);
        i += 1;
    }

    None
}

/// Extract contents inside backticks, respecting single quotes.
fn extract_backtick_contents(command: &str) -> Vec<String> {
    let mut results = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            i += 1;
            continue;
        }

        let ch = chars[i];

        if ch == '\\' && !in_single_quote {
            escape_next = true;
            i += 1;
            continue;
        }

        if ch == '\'' {
            in_single_quote = !in_single_quote;
            i += 1;
            continue;
        }

        if ch == '`' && !in_single_quote {
            // Find the matching closing backtick
            let start = i + 1;
            let mut j = start;
            let mut inner = String::new();
            let mut bt_escape = false;
            while j < len {
                if bt_escape {
                    bt_escape = false;
                    inner.push(chars[j]);
                    j += 1;
                    continue;
                }
                if chars[j] == '\\' {
                    bt_escape = true;
                    inner.push(chars[j]);
                    j += 1;
                    continue;
                }
                if chars[j] == '`' {
                    results.push(inner);
                    i = j + 1;
                    break;
                }
                inner.push(chars[j]);
                j += 1;
            }
            if j >= len {
                // Unclosed backtick — skip
                i = len;
            }
            continue;
        }

        i += 1;
    }

    results
}

fn is_segment_boundary(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | ';' | '&' | '|' | '\n' | '(' | ')')
}

/// Given a potentially path-qualified command, extract just the binary name.
fn extract_command_name(word: &str) -> &str {
    // Handle /usr/bin/ls, ./script, etc.
    word.rsplit('/').next().unwrap_or(word)
}

// ---------------------------------------------------------------------------
// Non-configurable context-sensitive classifiers (argument-dependent logic)
// ---------------------------------------------------------------------------

fn classify_sed(args: &[&str]) -> CommandClassification {
    if args.iter().any(|a| *a == "-i" || a.starts_with("-i")) {
        CommandClassification::Write
    } else {
        CommandClassification::ReadOnly
    }
}

fn classify_python(args: &[&str]) -> CommandClassification {
    // python -c "expression" — could be read or write, classify as Write to be safe
    // python -c with only print/import → could be read-only, but too complex to detect
    // python script.py → Write
    if args.first().copied() == Some("-c") {
        return CommandClassification::Write;
    }
    if args.first().copied() == Some("--version") || args.first().copied() == Some("-V") {
        return CommandClassification::ReadOnly;
    }
    if args.first().copied() == Some("--help") || args.first().copied() == Some("-h") {
        return CommandClassification::ReadOnly;
    }
    CommandClassification::Write
}

fn classify_node(args: &[&str]) -> CommandClassification {
    if args.first().copied() == Some("-e") || args.first().copied() == Some("--eval") {
        return CommandClassification::Write;
    }
    if args.first().copied() == Some("--version") || args.first().copied() == Some("-v") {
        return CommandClassification::ReadOnly;
    }
    if args.first().copied() == Some("--help") || args.first().copied() == Some("-h") {
        return CommandClassification::ReadOnly;
    }
    CommandClassification::Write
}

fn classify_tee(args: &[&str]) -> CommandClassification {
    // tee with no file arguments is just stdout passthrough → ReadOnly
    // tee with file arguments → Write
    let has_file_arg = args.iter().any(|a| !a.starts_with('-'));
    if has_file_arg {
        CommandClassification::Write
    } else {
        CommandClassification::ReadOnly
    }
}


// ---------------------------------------------------------------------------
// Shell tokenization helpers
// ---------------------------------------------------------------------------

/// Split a command string into segments on shell operators (`;`, `&&`, `||`, `|`, `\n`)
/// while respecting quoting.
fn split_command_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = command.chars().collect();
    let len = chars.len();
    let mut i = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;

    while i < len {
        if escape_next {
            escape_next = false;
            current.push(chars[i]);
            i += 1;
            continue;
        }

        let ch = chars[i];

        if ch == '\\' && !in_single_quote {
            escape_next = true;
            current.push(ch);
            i += 1;
            continue;
        }

        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            current.push(ch);
            i += 1;
            continue;
        }

        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            current.push(ch);
            i += 1;
            continue;
        }

        if in_single_quote || in_double_quote {
            current.push(ch);
            i += 1;
            continue;
        }

        // Newline → segment break
        if ch == '\n' {
            push_segment(&mut segments, &mut current);
            i += 1;
            continue;
        }

        // ; → segment break
        if ch == ';' {
            push_segment(&mut segments, &mut current);
            i += 1;
            continue;
        }

        // && or & (background)
        if ch == '&' {
            if i + 1 < len && chars[i + 1] == '&' {
                push_segment(&mut segments, &mut current);
                i += 2;
                continue;
            }
            // Single & (background) — treat as segment break
            push_segment(&mut segments, &mut current);
            i += 1;
            continue;
        }

        // || or | (pipe)
        if ch == '|' {
            if i + 1 < len && chars[i + 1] == '|' {
                push_segment(&mut segments, &mut current);
                i += 2;
                continue;
            }
            // Pipe
            push_segment(&mut segments, &mut current);
            i += 1;
            continue;
        }

        current.push(ch);
        i += 1;
    }

    push_segment(&mut segments, &mut current);
    segments
}

fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        segments.push(trimmed);
    }
    current.clear();
}

/// Simple segment splitter for sanitization (less precise, just splits on obvious operators).
fn split_segments_simple(command: &str) -> Vec<&str> {
    // Split on newlines, semicolons, &&, ||, |
    // This is a rough split — doesn't handle quoting, but good enough for
    // sanitization where we just need to find the first word of each segment.
    let mut segments = Vec::new();
    let mut start = 0;
    let bytes = command.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        let b = bytes[i];
        if b == b'\n' || b == b';' {
            let seg = &command[start..i];
            if !seg.trim().is_empty() {
                segments.push(seg.trim());
            }
            start = i + 1;
        } else if b == b'&' || b == b'|' {
            let seg = &command[start..i];
            if !seg.trim().is_empty() {
                segments.push(seg.trim());
            }
            // Skip doubled operators
            if i + 1 < len && bytes[i + 1] == b {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }

    let remaining = &command[start..];
    if !remaining.trim().is_empty() {
        segments.push(remaining.trim());
    }

    segments
}

/// Extract the first word of a string.
fn first_word_of(s: &str) -> &str {
    s.split_whitespace().next().unwrap_or("")
}

/// Extract unquoted tokens from a command (for pattern scanning).
fn unquoted_tokens(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut escape_next = false;

    for ch in command.chars() {
        if escape_next {
            escape_next = false;
            // Don't add escaped chars to unquoted tokens
            continue;
        }

        if ch == '\\' && !in_single_quote {
            escape_next = true;
            continue;
        }

        if ch == '\'' && !in_double_quote {
            in_single_quote = !in_single_quote;
            continue;
        }

        if ch == '"' && !in_single_quote {
            in_double_quote = !in_double_quote;
            continue;
        }

        if in_single_quote || in_double_quote {
            continue;
        }

        if ch.is_whitespace() {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

/// Split a segment into words, respecting quotes (for argument parsing).
fn shell_words(segment: &str) -> Vec<&str> {
    // Simple word split — for classification we just need to identify the
    // command name and key flags, so splitting on whitespace is sufficient.
    // Quoted arguments are kept together as-is.
    segment.split_whitespace().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Sanitization ---

    #[test]
    fn cs01_empty_command_rejected() {
        assert_eq!(
            sanitize(""),
            SanitizeResult::Rejected {
                reason: "empty command".to_string()
            }
        );
        assert_eq!(
            sanitize("   "),
            SanitizeResult::Rejected {
                reason: "empty command".to_string()
            }
        );
    }

    #[test]
    fn cs02_null_byte_rejected() {
        assert_eq!(
            sanitize("ls\0 -la"),
            SanitizeResult::Rejected {
                reason: "command contains null byte".to_string()
            }
        );
    }

    #[test]
    fn cs03_fork_bomb_rejected() {
        assert_eq!(
            sanitize(":(){ :|:& };:"),
            SanitizeResult::Rejected {
                reason: "fork bomb detected".to_string()
            }
        );
    }

    #[test]
    fn cs04_privilege_escalation_rejected() {
        assert_eq!(
            sanitize("sudo rm -rf /"),
            SanitizeResult::Rejected {
                reason: "privilege escalation commands are not allowed".to_string()
            }
        );
        assert_eq!(
            sanitize("su -c 'rm -rf /'"),
            SanitizeResult::Rejected {
                reason: "privilege escalation commands are not allowed".to_string()
            }
        );
        assert_eq!(
            sanitize("doas apt install foo"),
            SanitizeResult::Rejected {
                reason: "privilege escalation commands are not allowed".to_string()
            }
        );
    }

    #[test]
    fn cs05_raw_device_access_rejected() {
        assert_eq!(
            sanitize("dd if=/dev/sda of=disk.img"),
            SanitizeResult::Rejected {
                reason: "raw device access is not allowed".to_string()
            }
        );
    }

    #[test]
    fn cs06_valid_commands_pass() {
        assert_eq!(sanitize("ls -la"), SanitizeResult::Ok);
        assert_eq!(sanitize("git status"), SanitizeResult::Ok);
        assert_eq!(sanitize("cargo test"), SanitizeResult::Ok);
        assert_eq!(sanitize("rm -rf /tmp/test"), SanitizeResult::Ok);
    }

    #[test]
    fn cs_kernel_module_rejected() {
        assert_eq!(
            sanitize("insmod malicious.ko"),
            SanitizeResult::Rejected {
                reason: "kernel module manipulation is not allowed".to_string()
            }
        );
        assert_eq!(
            sanitize("modprobe vfio"),
            SanitizeResult::Rejected {
                reason: "kernel module manipulation is not allowed".to_string()
            }
        );
    }

    #[test]
    fn cs_sudo_in_chained_command_rejected() {
        assert_eq!(
            sanitize("ls && sudo rm -rf /"),
            SanitizeResult::Rejected {
                reason: "privilege escalation commands are not allowed".to_string()
            }
        );
    }

    // --- Classification: ReadOnly ---

    #[test]
    fn cc01_ls_is_readonly() {
        assert_eq!(classify("ls -la"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc02_cat_is_readonly() {
        assert_eq!(classify("cat file.txt"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc03_grep_is_readonly() {
        assert_eq!(
            classify("grep -rn pattern src/"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc04_git_status_is_readonly() {
        assert_eq!(classify("git status"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc05_git_log_is_readonly() {
        assert_eq!(
            classify("git log --oneline -20"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc06_cargo_test_is_readonly() {
        assert_eq!(
            classify("cargo test --workspace"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc07_cargo_check_is_readonly() {
        assert_eq!(classify("cargo check"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc08_find_is_readonly() {
        assert_eq!(
            classify("find . -name '*.rs'"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc09_pwd_echo_readonly() {
        assert_eq!(classify("pwd"), CommandClassification::ReadOnly);
        assert_eq!(classify("echo hello"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc10_diff_is_readonly() {
        assert_eq!(classify("diff a.txt b.txt"), CommandClassification::ReadOnly);
    }

    // --- Classification: Write ---

    #[test]
    fn cc11_touch_is_write() {
        assert_eq!(classify("touch newfile.txt"), CommandClassification::Write);
    }

    #[test]
    fn cc12_mkdir_is_write() {
        assert_eq!(classify("mkdir -p src/new"), CommandClassification::Write);
    }

    #[test]
    fn cc13_sed_i_is_write() {
        assert_eq!(
            classify("sed -i 's/foo/bar/' file.txt"),
            CommandClassification::Write
        );
    }

    #[test]
    fn cc14_npm_install_is_write() {
        assert_eq!(classify("npm install"), CommandClassification::Write);
    }

    #[test]
    fn cc15_git_commit_is_write() {
        assert_eq!(
            classify("git commit -m 'msg'"),
            CommandClassification::Write
        );
    }

    #[test]
    fn cc16_cargo_build_is_write() {
        assert_eq!(classify("cargo build"), CommandClassification::Write);
    }

    #[test]
    fn cc17_cp_is_write() {
        assert_eq!(classify("cp src.txt dst.txt"), CommandClassification::Write);
    }

    #[test]
    fn cc18_mv_is_write() {
        assert_eq!(classify("mv old.txt new.txt"), CommandClassification::Write);
    }

    #[test]
    fn cc19_curl_is_write() {
        assert_eq!(
            classify("curl https://example.com"),
            CommandClassification::Write
        );
    }

    #[test]
    fn cc20_git_add_is_write() {
        assert_eq!(classify("git add ."), CommandClassification::Write);
    }

    // --- Classification: Destructive ---

    #[test]
    fn cc21_rm_is_destructive() {
        assert_eq!(classify("rm file.txt"), CommandClassification::Destructive);
    }

    #[test]
    fn cc22_rm_rf_is_destructive() {
        assert_eq!(
            classify("rm -rf /tmp/test"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc23_git_clean_is_destructive() {
        assert_eq!(
            classify("git clean -fd"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc24_git_reset_hard_is_destructive() {
        assert_eq!(
            classify("git reset --hard HEAD~1"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc25_dd_is_destructive() {
        assert_eq!(
            classify("dd if=/dev/zero of=file bs=1M count=1"),
            CommandClassification::Destructive
        );
    }

    // --- Classification: Chained commands ---

    #[test]
    fn cc26_chain_ls_and_rm() {
        assert_eq!(
            classify("ls && rm -rf /tmp/x"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc27_pipe_readonly() {
        assert_eq!(
            classify("cat file | grep pattern"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc28_cd_semicolon_ls() {
        assert_eq!(classify("cd /tmp; ls"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc29_redirect_write() {
        // > is destructive (overwrite), but echo itself is readonly
        // The redirect escalates to Destructive
        assert_eq!(
            classify("echo hello > file.txt"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc29b_append_redirect_write() {
        assert_eq!(
            classify("echo hello >> file.txt"),
            CommandClassification::Write
        );
    }

    #[test]
    fn cc30_command_substitution_destructive() {
        assert_eq!(
            classify("cat $(rm secret.txt)"),
            CommandClassification::Destructive
        );
    }

    // --- Context-sensitive ---

    #[test]
    fn cc31_git_diff_readonly() {
        assert_eq!(
            classify("git diff HEAD~1"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc32_git_push_force_destructive() {
        assert_eq!(
            classify("git push --force origin main"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc33_cargo_clippy_readonly() {
        assert_eq!(
            classify("cargo clippy --workspace --tests"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc34_cargo_clean_destructive() {
        assert_eq!(
            classify("cargo clean"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc35_sed_without_i_readonly() {
        assert_eq!(
            classify("sed 's/foo/bar/' file.txt"),
            CommandClassification::ReadOnly
        );
    }

    // --- Unknown defaults to Write ---

    #[test]
    fn cc36_unknown_is_write() {
        assert_eq!(
            classify("my_custom_script.sh"),
            CommandClassification::Write
        );
    }

    #[test]
    fn cc37_unknown_binary_is_write() {
        assert_eq!(
            classify("/usr/local/bin/something"),
            CommandClassification::Write
        );
    }

    // --- Edge cases ---

    #[test]
    fn cc41_quoted_rm_is_readonly() {
        // "rm -rf /" is inside quotes — echo is the command
        assert_eq!(
            classify(r#"echo "rm -rf /""#),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc42_newline_separated_commands() {
        assert_eq!(
            classify("ls\nrm file"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc43_subshell_destructive() {
        assert_eq!(
            classify("(cd /tmp && rm -rf foo)"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc44_deeply_nested_substitution() {
        assert_eq!(
            classify("echo $(echo $(rm file))"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc45_process_substitution_readonly() {
        assert_eq!(
            classify("diff <(ls dir1) <(ls dir2)"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc46_npm_test_readonly() {
        assert_eq!(classify("npm test"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc47_npm_run_lint_readonly() {
        assert_eq!(classify("npm run lint"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc48_npx_tsc_noemit_readonly() {
        assert_eq!(
            classify("npx tsc --noEmit"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc49_tee_with_file_is_write() {
        assert_eq!(
            classify("ls | tee output.txt"),
            CommandClassification::Write
        );
    }

    #[test]
    fn cc50_tee_without_file_is_readonly() {
        assert_eq!(classify("ls | tee"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cc51_xargs_with_rm_is_destructive() {
        assert_eq!(
            classify("find . -name '*.tmp' | xargs rm"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc52_xargs_with_no_command_is_readonly() {
        assert_eq!(
            classify("echo hello | xargs"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc53_git_checkout_dot_is_destructive() {
        assert_eq!(
            classify("git checkout ."),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc54_python_version_is_readonly() {
        assert_eq!(
            classify("python --version"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc55_path_qualified_ls_is_readonly() {
        assert_eq!(
            classify("/usr/bin/ls -la"),
            CommandClassification::ReadOnly
        );
    }

    #[test]
    fn cc56_backgrounded_command() {
        // ls & rm → max(ReadOnly, Destructive) = Destructive
        assert_eq!(
            classify("ls & rm file"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc57_or_chain() {
        assert_eq!(
            classify("false || rm file"),
            CommandClassification::Destructive
        );
    }

    #[test]
    fn cc58_backtick_substitution() {
        assert_eq!(
            classify("echo `rm file`"),
            CommandClassification::Destructive
        );
    }

    // --- Display ---

    #[test]
    fn classification_display() {
        assert_eq!(CommandClassification::ReadOnly.to_string(), "read_only");
        assert_eq!(CommandClassification::Write.to_string(), "write");
        assert_eq!(CommandClassification::Destructive.to_string(), "destructive");
    }

    // --- Custom config tests ---

    #[test]
    fn cfg01_custom_read_only_command() {
        let mut config = CommandClassifierConfig::default();
        config.read_only_commands.push("mytool".to_string());
        let classifier = CommandClassifier::new(config);
        assert_eq!(classifier.classify("mytool --flag"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cfg02_removed_from_read_only_defaults_to_write() {
        let config = CommandClassifierConfig {
            read_only_commands: vec!["cat".to_string()],
            ..CommandClassifierConfig::default()
        };
        let classifier = CommandClassifier::new(config);
        // "ls" was removed from read_only, so it defaults to Write
        assert_eq!(classifier.classify("ls"), CommandClassification::Write);
        // "cat" is still read-only
        assert_eq!(classifier.classify("cat file"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cfg03_move_rm_to_write_list() {
        let mut config = CommandClassifierConfig::default();
        config.destructive_commands.retain(|c| c != "rm");
        config.write_commands.push("rm".to_string());
        let classifier = CommandClassifier::new(config);
        assert_eq!(classifier.classify("rm file"), CommandClassification::Write);
    }

    #[test]
    fn cfg04_custom_git_subcommands() {
        let mut config = CommandClassifierConfig::default();
        config.git_read_only_subcommands.push("stash".to_string());
        let classifier = CommandClassifier::new(config);
        assert_eq!(classifier.classify("git stash"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cfg05_custom_cargo_destructive() {
        let mut config = CommandClassifierConfig::default();
        config.cargo_destructive_subcommands.push("publish".to_string());
        let classifier = CommandClassifier::new(config);
        assert_eq!(classifier.classify("cargo publish"), CommandClassification::Destructive);
    }

    #[test]
    fn cfg06_custom_npm_read_only_scripts() {
        let mut config = CommandClassifierConfig::default();
        config.npm_read_only_scripts.push("format".to_string());
        let classifier = CommandClassifier::new(config);
        assert_eq!(classifier.classify("npm run format"), CommandClassification::ReadOnly);
    }

    #[test]
    fn cfg07_empty_lists_everything_defaults_to_write() {
        let config = CommandClassifierConfig {
            read_only_commands: vec![],
            write_commands: vec![],
            destructive_commands: vec![],
            git_read_only_subcommands: vec![],
            git_destructive_subcommands: vec![],
            cargo_read_only_subcommands: vec![],
            cargo_destructive_subcommands: vec![],
            npm_read_only_subcommands: vec![],
            npm_read_only_scripts: vec![],
        };
        let classifier = CommandClassifier::new(config);
        assert_eq!(classifier.classify("ls"), CommandClassification::Write);
        assert_eq!(classifier.classify("rm file"), CommandClassification::Write);
        assert_eq!(classifier.classify("git status"), CommandClassification::Write);
        assert_eq!(classifier.classify("cargo test"), CommandClassification::Write);
    }

    #[test]
    fn cfg08_serde_roundtrip_toml() {
        let config = CommandClassifierConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: CommandClassifierConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(config.read_only_commands, deserialized.read_only_commands);
        assert_eq!(config.write_commands, deserialized.write_commands);
        assert_eq!(config.destructive_commands, deserialized.destructive_commands);
        assert_eq!(config.git_read_only_subcommands, deserialized.git_read_only_subcommands);
    }

    #[test]
    fn cfg09_partial_toml_missing_fields_get_defaults() {
        let partial = r#"
read_only_commands = ["ls", "cat"]
"#;
        let config: CommandClassifierConfig = toml::from_str(partial).unwrap();
        assert_eq!(config.read_only_commands, vec!["ls", "cat"]);
        // Other fields should be defaults
        let defaults = CommandClassifierConfig::default();
        assert_eq!(config.write_commands, defaults.write_commands);
        assert_eq!(config.destructive_commands, defaults.destructive_commands);
        assert_eq!(config.git_read_only_subcommands, defaults.git_read_only_subcommands);
    }

    #[test]
    fn cfg10_classifier_with_defaults_matches_free_function() {
        let classifier = CommandClassifier::with_defaults();
        // Spot-check that the struct-based classifier matches the free function
        assert_eq!(classifier.classify("ls -la"), classify("ls -la"));
        assert_eq!(classifier.classify("rm -rf /tmp"), classify("rm -rf /tmp"));
        assert_eq!(classifier.classify("git status"), classify("git status"));
        assert_eq!(classifier.classify("cargo build"), classify("cargo build"));
        assert_eq!(classifier.classify("npm test"), classify("npm test"));
    }
}
