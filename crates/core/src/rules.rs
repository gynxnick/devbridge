//! Default sync exclusion rules (gitignore syntax). These are the single source
//! of truth for both the sync engine and the Settings UI.

/// Always excluded; cannot be opted in. `.git` history moves through Git
/// remotes, and `.devbridge` is DevBridge's own staging area.
pub const ALWAYS_EXCLUDED: &[&str] = &[".git/", ".devbridge/"];

/// Generated, machine-specific or editor-temporary files. Users can override
/// these with `!pattern` rules in their own exclusion list.
pub const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules/",
    "target/",
    "dist/",
    "build/",
    ".next/",
    ".nuxt/",
    ".turbo/",
    ".cache/",
    ".gradle/",
    ".venv/",
    "venv/",
    "__pycache__/",
    "*.pyc",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    "*.swp",
    "*.swo",
    "*~",
    "~$*",
    ".#*",
    "*.tmp",
];

/// Likely secrets or machine-specific credentials. Never synced unless the user
/// explicitly opts a pattern in (recorded as an audit event).
pub const DEFAULT_SECRET_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    "*.keystore",
    "*.jks",
    ".npmrc",
    ".pypirc",
    ".netrc",
    "credentials*.json",
    "secrets.*",
    ".aws/",
    ".ssh/",
    ".claude/settings.local.json",
];

/// Exceptions to the secret patterns that are conventionally safe templates.
pub const SECRET_EXCEPTIONS: &[&str] = &["!.env.example", "!.env.sample", "!.env.template"];
