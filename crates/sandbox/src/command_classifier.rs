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

use std::fmt;

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

/// Classify a command as ReadOnly, Write, or Destructive.
///
/// Splits on shell operators, handles quoting, command substitution, subshells,
/// and redirects. Returns the maximum classification across all segments.
pub fn classify(command: &str) -> CommandClassification {
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
        let class = classify_single_segment(segment);
        max_class = max_class.max(class);
    }

    // Check for command substitution, subshells, process substitution
    max_class = max_class.max(classify_nested(trimmed));

    max_class
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

/// Extract and classify nested constructs: $(...), `...`, (...), >(...)
fn classify_nested(command: &str) -> CommandClassification {
    let mut max_class = CommandClassification::ReadOnly;

    // Extract $(...) and >(...) contents — handle nesting via depth tracking
    for inner in extract_parenthesized_contents(command) {
        let inner_class = classify(&inner);
        max_class = max_class.max(inner_class);
    }

    // Extract backtick contents
    for inner in extract_backtick_contents(command) {
        let inner_class = classify(&inner);
        max_class = max_class.max(inner_class);
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

/// Classify a single simple command segment (no shell operators).
fn classify_single_segment(segment: &str) -> CommandClassification {
    let words: Vec<&str> = shell_words(segment);
    if words.is_empty() {
        return CommandClassification::ReadOnly;
    }

    let command_name = extract_command_name(words[0]);
    let args = &words[1..];

    classify_command_with_args(command_name, args)
}

/// Given a potentially path-qualified command, extract just the binary name.
fn extract_command_name(word: &str) -> &str {
    // Handle /usr/bin/ls, ./script, etc.
    word.rsplit('/').next().unwrap_or(word)
}

/// Classify based on command name and its arguments.
fn classify_command_with_args(command: &str, args: &[&str]) -> CommandClassification {
    // Context-sensitive commands first
    match command {
        "git" => return classify_git(args),
        "cargo" => return classify_cargo(args),
        "npm" | "npx" => return classify_npm(command, args),
        "sed" => return classify_sed(args),
        "python" | "python3" => return classify_python(args),
        "node" => return classify_node(args),
        "tee" => return classify_tee(args),
        "xargs" => return classify_xargs(args),
        _ => {}
    }

    // Simple read-only commands
    if is_read_only_command(command) {
        return CommandClassification::ReadOnly;
    }

    // Destructive commands
    if is_destructive_command(command) {
        return CommandClassification::Destructive;
    }

    // Known write commands
    if is_write_command(command) {
        return CommandClassification::Write;
    }

    // Unknown commands default to Write
    CommandClassification::Write
}

fn is_read_only_command(command: &str) -> bool {
    matches!(
        command,
        "cd"
            | "ls"
            | "cat"
            | "head"
            | "tail"
            | "less"
            | "more"
            | "wc"
            | "file"
            | "find"
            | "grep"
            | "egrep"
            | "fgrep"
            | "rg"
            | "ag"
            | "awk"
            | "gawk"
            | "which"
            | "whereis"
            | "type"
            | "echo"
            | "printf"
            | "pwd"
            | "env"
            | "printenv"
            | "whoami"
            | "id"
            | "hostname"
            | "uname"
            | "date"
            | "cal"
            | "uptime"
            | "df"
            | "du"
            | "free"
            | "top"
            | "htop"
            | "ps"
            | "stat"
            | "readlink"
            | "realpath"
            | "basename"
            | "dirname"
            | "test"
            | "["
            | "true"
            | "false"
            | "diff"
            | "cmp"
            | "md5sum"
            | "sha256sum"
            | "sha1sum"
            | "sha512sum"
            | "xxd"
            | "od"
            | "strings"
            | "tree"
            | "bat"
            | "jq"
            | "yq"
            | "sort"
            | "uniq"
            | "cut"
            | "tr"
            | "column"
            | "comm"
            | "join"
            | "paste"
            | "fold"
            | "rev"
            | "tac"
            | "nl"
            | "expand"
            | "unexpand"
            | "hexdump"
            | "man"
            | "help"
            | "info"
    )
}

fn is_write_command(command: &str) -> bool {
    matches!(
        command,
        "touch"
            | "mkdir"
            | "cp"
            | "mv"
            | "chmod"
            | "chown"
            | "chgrp"
            | "curl"
            | "wget"
            | "tar"
            | "unzip"
            | "zip"
            | "gzip"
            | "gunzip"
            | "bzip2"
            | "bunzip2"
            | "xz"
            | "unxz"
            | "make"
            | "cmake"
            | "patch"
            | "ln"
    )
}

fn is_destructive_command(command: &str) -> bool {
    matches!(
        command,
        "rm" | "rmdir" | "dd" | "mkfs" | "shred" | "truncate"
    )
}

// ---------------------------------------------------------------------------
// Context-sensitive classifiers
// ---------------------------------------------------------------------------

fn classify_git(args: &[&str]) -> CommandClassification {
    let subcommand = args.first().copied().unwrap_or("");

    // Read-only git subcommands
    let read_only = [
        "status",
        "log",
        "diff",
        "show",
        "branch",
        "tag",
        "remote",
        "rev-parse",
        "ls-files",
        "ls-tree",
        "describe",
        "shortlog",
        "blame",
        "bisect",
        "reflog",
        "stash list",
        "config",
        "help",
        "version",
    ];
    if read_only.contains(&subcommand) {
        return CommandClassification::ReadOnly;
    }

    // Destructive git subcommands
    if subcommand == "clean" {
        return CommandClassification::Destructive;
    }
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

    // Write: add, commit, checkout (branch switch), merge, rebase, stash push/pop, pull, fetch, etc.
    CommandClassification::Write
}

fn classify_cargo(args: &[&str]) -> CommandClassification {
    let subcommand = args.first().copied().unwrap_or("");

    let read_only = ["check", "test", "clippy", "doc", "bench", "metadata", "tree", "version", "help"];
    if read_only.contains(&subcommand) {
        return CommandClassification::ReadOnly;
    }

    if subcommand == "clean" {
        return CommandClassification::Destructive;
    }

    // build, fmt, run, install, add, remove, update, publish, etc.
    CommandClassification::Write
}

fn classify_npm(command: &str, args: &[&str]) -> CommandClassification {
    let subcommand = args.first().copied().unwrap_or("");

    // npx with tsc/eslint/prettier --check is read-only
    if command == "npx" {
        let tool = args.first().copied().unwrap_or("");
        if tool == "tsc" && args.contains(&"--noEmit") {
            return CommandClassification::ReadOnly;
        }
        // npx unknown tool → Write
        return CommandClassification::Write;
    }

    let read_only_subcommands = ["test", "list", "ls", "view", "info", "outdated", "help", "version"];
    if read_only_subcommands.contains(&subcommand) {
        return CommandClassification::ReadOnly;
    }

    // npm run <script> — classify by script name
    if subcommand == "run" || subcommand == "run-script" {
        let script = args.get(1).copied().unwrap_or("");
        let read_only_scripts = ["test", "lint", "check", "typecheck", "type-check", "validate"];
        if read_only_scripts.contains(&script) {
            return CommandClassification::ReadOnly;
        }
    }

    if subcommand == "cache" && args.contains(&"clean") {
        return CommandClassification::Destructive;
    }

    // install, run build, run dev, ci, update, etc.
    CommandClassification::Write
}

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

fn classify_xargs(args: &[&str]) -> CommandClassification {
    // xargs with an explicit command → classify that command
    // Find the command after xargs flags
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "-I" || arg == "-L" || arg == "-n" || arg == "-P" || arg == "-d" {
            i += 2; // skip flag and its value
            continue;
        }
        if arg.starts_with('-') {
            i += 1;
            continue;
        }
        // This is the command xargs will run
        return classify_command_with_args(extract_command_name(arg), &args[i + 1..]);
    }
    // xargs with no explicit command defaults to echo → ReadOnly
    CommandClassification::ReadOnly
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
}
