# Workspace Inheritance Support Analysis

## Problem Statement

`cargo-sane` currently fails to parse `Cargo.toml` files that use workspace inheritance, a feature introduced in Cargo 1.64 (Oct 2022). This prevents the tool from working with modern Rust projects that use workspace dependency management.

### Error Reproduction

```bash
$ cargo sane check --manifest-path /tmp/test-workspace.toml
Error: Failed to parse Cargo.toml

Caused by:
    TOML parse error at line 3, column 1
      |
    3 | version.workspace = true
      | ^^^^^^^
    invalid type: map, expected a string
```

### Test Case

```toml
[package]
name = "test-package"
version.workspace = true
edition.workspace = true

[dependencies]
serde = "1.0"
```

## Root Cause

### Current Implementation

In `src/core/manifest.rs:26-29`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,  // ← Expects plain string
}
```

### Workspace Inheritance Syntax

The TOML dotted-key syntax `version.workspace = true` is equivalent to:

```toml
version = { workspace = true }
```

This creates a nested structure that Serde tries to deserialize into the `String` field, causing a type mismatch.

## Solution Design

### 1. Support Workspace Inheritance Types

Add new types to represent workspace-inheritable fields:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum InheritableString {
    Value(String),
    Workspace { workspace: bool },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: InheritableString,
    pub edition: Option<InheritableString>,
    pub authors: Option<InheritableString>,
    // ... other inheritable fields
}
```

### 2. Handle Dependency Version Inheritance

Dependencies can also inherit versions:

```toml
[dependencies]
serde.workspace = true  # Inherits version and features from workspace
```

Update `DependencySpec` to support this:

```rust
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum DependencySpec {
    Simple(String),
    Detailed(DetailedDependency),
    Workspace { workspace: bool },  // ← New variant
}
```

### 3. Workspace Root Detection

To resolve workspace-inherited values, we need to:

1. Detect if current manifest is part of a workspace
2. Find and parse workspace root `Cargo.toml`
3. Extract inherited values from `[workspace.package]` and `[workspace.dependencies]`

```rust
pub struct WorkspaceRoot {
    pub path: PathBuf,
    pub package: Option<WorkspacePackage>,
    pub dependencies: Option<HashMap<String, DependencySpec>>,
}

impl Manifest {
    pub fn find_workspace_root(&self) -> Result<Option<WorkspaceRoot>> {
        // Walk up directory tree looking for workspace Cargo.toml
    }

    pub fn resolve_version(&self) -> Result<String> {
        match &self.content.package.version {
            InheritableString::Value(v) => Ok(v.clone()),
            InheritableString::Workspace { .. } => {
                // Look up workspace root and get version
            }
        }
    }
}
```

## Implementation Strategy

### Phase 1: Basic Parsing (Minimal Fix)

**Goal:** Allow parsing without resolution

1. Update `Package` struct to accept workspace inheritance syntax
2. Update `DependencySpec` to accept workspace inheritance
3. For now, skip dependencies that use `workspace = true`
4. This allows the tool to work on workspace members, but only for non-inherited deps

**Files to modify:**
- `src/core/manifest.rs` - Add inheritable types
- `src/analyzer/checker.rs` - Skip workspace-inherited dependencies

**Estimated effort:** 2-3 hours

### Phase 2: Full Workspace Support

**Goal:** Resolve inherited values from workspace root

1. Implement workspace root detection
2. Parse workspace root `Cargo.toml`
3. Resolve inherited package fields (version, edition, etc.)
4. Resolve inherited dependency versions
5. Handle workspace-wide updates

**Files to modify:**
- `src/core/manifest.rs` - Add workspace resolution
- `src/core/workspace.rs` (new) - Workspace handling
- `src/analyzer/checker.rs` - Use resolved values
- `src/updater/update.rs` - Update workspace root when needed

**Estimated effort:** 1-2 days

## Testing Strategy

### Unit Tests

1. Parse workspace-inherited package fields
2. Parse workspace-inherited dependencies
3. Workspace root detection
4. Value resolution from workspace root

### Integration Tests

1. Check command on workspace member
2. Update command on workspace member
3. Multi-member workspace updates
4. Mixed inherited and non-inherited dependencies

### Test Fixtures

Create test workspace structure:

```
tests/fixtures/workspace/
├── Cargo.toml (workspace root)
├── member1/
│   └── Cargo.toml (uses version.workspace = true)
└── member2/
    └── Cargo.toml (uses dependency.workspace = true)
```

## References

- [Cargo Workspace Documentation](https://doc.rust-lang.org/cargo/reference/workspaces.html)
- [RFC 2906: Workspace Inheritance](https://rust-lang.github.io/rfcs/2906-cargo-workspace-deduplicate.html)
- [Issue #1: support flattened toml project configuration](https://github.com/chronocoders/cargo-sane/issues/1)

## Success Criteria

- ✅ Parse `Cargo.toml` with `version.workspace = true`
- ✅ Parse `Cargo.toml` with `dependency.workspace = true`
- ✅ Skip workspace-inherited dependencies gracefully (Phase 1)
- ✅ Resolve inherited values from workspace root (Phase 2)
- ✅ Update workspace dependencies correctly (Phase 2)
- ✅ All existing tests still pass
- ✅ New integration tests for workspace scenarios
