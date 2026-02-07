# remix-credentials — Product Requirements Document

**Version:** 0.1.0-draft
**Date:** February 7, 2026
**Status:** Ready for development — build this before remix-agent-runtime

---

## Business Context

remix-credentials is the trust foundation for the entire remix-agent ecosystem. It is the only code that ever touches a user's raw credentials (site passwords, API keys, tokens). Both the open source runtime and the closed source managed platform depend on this crate for all credential operations.

### Why This Exists as a Separate Repo

This library is intentionally small, focused, and isolated so that:

- Security researchers can audit the entire codebase in an afternoon
- Users can verify exactly how their credentials are handled without reading the full runtime or platform source
- The trust story is simple: "Here's one repo. It handles all credential crypto. Read every line."

### How It Fits Into the Ecosystem

```
remix-credentials (this crate, MIT, open source)
    ▲                       ▲
    │                       │
    │ depends on            │ depends on
    │                       │
remix-agent-runtime     remix-agent-platform
(MIT, open source)      (proprietary, closed source)
```

The runtime uses this crate to load and decrypt credentials locally. The platform uses the same crate inside its vault service to encrypt credentials at rest and decrypt them inside agent containers. Same crypto code path regardless of where the agent runs. No proprietary credential handling anywhere in the stack.

### Design Principles

1. **Do one thing.** Encrypt, decrypt, and zeroize credentials. Nothing else. No storage, no networking, no key management infrastructure, no access control.
2. **No secrets in, no secrets out.** The crate never writes credentials to disk, never logs them, never sends them anywhere. It operates purely in memory.
3. **Standard crypto only.** Use well-known, audited algorithms. No custom cryptography. AES-256-GCM for encryption, Argon2id for key derivation when needed.
4. **Zeroize everything.** All sensitive memory is overwritten on drop. No credential material survives in memory after use.
5. **Minimal dependencies.** Every dependency is attack surface. Use only well-audited crates from the RustCrypto project where possible.

---

## Functional Requirements

### R1: Credential Data Model

Define a standard credential structure that both the runtime and platform use.

**Must:**

- Support a `Credential` type with the following fields:
  - `name` — identifier for the credential (e.g., "github_login")
  - `credential_type` — enum: `UsernamePassword`, `ApiKey`, `Token`, `Cookie`, `Custom`
  - `fields` — key-value map of credential data (e.g., `username`, `password`, `token`)
  - `url_pattern` — optional glob pattern for associated URLs (e.g., "*.github.com")
  - `metadata` — optional key-value map for non-sensitive context (e.g., "created_at", "label")
- All fields containing sensitive data must implement the `Zeroize` and `ZeroizeOnDrop` traits
- Serialize to and deserialize from JSON (for passing between runtime components)
- The `Display` and `Debug` trait implementations must redact all sensitive fields

**Should:**

- Support a `CredentialSet` type that holds multiple credentials for a single agent run
- Validate credential completeness on construction (e.g., `UsernamePassword` requires both `username` and `password` fields)

### R2: Encryption

Encrypt credentials for storage. The platform stores encrypted blobs in its database. Users can also store encrypted credential files locally.

**Must:**

- Encrypt credential data using AES-256-GCM
- Generate a unique random nonce (96-bit) per encryption operation — never reuse nonces
- Accept an encryption key as a 256-bit byte array — the crate does not decide where keys come from
- Return an `EncryptedCredential` struct containing: ciphertext, nonce, and an algorithm version tag
- The algorithm version tag must be included so future versions can migrate to new algorithms without breaking existing encrypted data
- Serialize `EncryptedCredential` to and from bytes (for storage) and base64 (for config files)

**Should:**

- Support encrypting a full `CredentialSet` as a single encrypted blob
- Include an integrity check — the GCM authentication tag validates that ciphertext has not been tampered with (this is inherent to AES-GCM but must not be stripped or ignored)

**Won't:**

- Manage encryption keys — the caller provides them
- Store encrypted data — the caller decides where it goes
- Implement asymmetric encryption (RSA, EC) — not needed for this use case

### R3: Decryption

Decrypt credentials for use during an agent run.

**Must:**

- Decrypt `EncryptedCredential` back to a `Credential` or `CredentialSet`
- Accept the decryption key as a 256-bit byte array
- Return a clear error if decryption fails (wrong key, tampered data, corrupted blob) — do not return partial data
- The decrypted `Credential` must have `ZeroizeOnDrop` so it is wiped from memory when it goes out of scope
- Zeroize the decryption key material after use if the caller passes ownership

**Should:**

- Support a `DecryptedCredentialGuard` wrapper type that:
  - Provides read access to credential fields
  - Automatically zeroizes on drop
  - Cannot be cloned, serialized, or leaked outside the guard
  - Has a configurable TTL (time-to-live) after which it zeroizes automatically even if not dropped

### R4: Key Derivation

Derive encryption keys from user-provided passphrases when hardware key management is not available (local development use case).

**Must:**

- Derive a 256-bit key from a passphrase using Argon2id
- Use a random 128-bit salt per derivation
- Use secure default parameters (memory: 64MB, iterations: 3, parallelism: 4) that balance security and performance
- Return the derived key and salt (salt must be stored alongside encrypted data for later decryption)

**Should:**

- Accept configurable Argon2id parameters for environments with different resource constraints
- Include a `DerivedKey` type that zeroizes on drop

**Won't:**

- Integrate with external key management systems (AWS KMS, HashiCorp Vault, etc.) — the platform builds that integration on top of this crate

### R5: Credential Loading from Environment

Support loading credentials from environment variables, which is the primary mechanism for the runtime.

**Must:**

- Load credential values from environment variables by name
- Clear the environment variable after reading (to reduce exposure window)
- Return the loaded credential wrapped in a zeroizing type

**Should:**

- Support a naming convention for auto-discovery: `REMIX_CRED_{NAME}_{FIELD}` (e.g., `REMIX_CRED_GITHUB_USERNAME`, `REMIX_CRED_GITHUB_PASSWORD`)
- Support loading a `CredentialSet` from a set of related environment variables

### R6: Credential File Format

Support loading credentials from an encrypted file for local workflows.

**Must:**

- Define a file format for storing encrypted credentials (JSON with base64-encoded encrypted blob + salt + algorithm version)
- Decrypt credential files given a passphrase (using R4 key derivation + R3 decryption)
- Provide a CLI-friendly function to create encrypted credential files from plaintext input

**Should:**

- The file format must be versioned so it can evolve without breaking existing files
- Support the following file structure:

```json
{
  "version": 1,
  "algorithm": "aes-256-gcm",
  "kdf": "argon2id",
  "kdf_params": {
    "memory_kb": 65536,
    "iterations": 3,
    "parallelism": 4
  },
  "salt": "<base64>",
  "nonce": "<base64>",
  "ciphertext": "<base64>"
}
```

---

## Non-Functional Requirements

### Security

- All sensitive memory must be zeroized on drop — no exceptions
- No logging of credential values at any log level, including debug and trace
- No `Clone` implementation on any type that holds decrypted credential data
- All randomness must come from a cryptographically secure source (`rand::rngs::OsRng`)
- The crate must not use `unsafe` code unless absolutely necessary, and any `unsafe` blocks must be documented with a safety justification

### Testing

- Unit tests for every encrypt/decrypt round-trip
- Tests verifying that wrong keys produce decryption errors, not garbage data
- Tests verifying zeroization (check memory contents after drop where possible)
- Tests verifying that `Debug` and `Display` output never contains sensitive data
- Fuzz testing on the deserialization paths (encrypted credential parsing)
- Integration test that demonstrates the full flow: create credential → encrypt → serialize → deserialize → decrypt → use → drop → verify zeroized

### Documentation

- Every public type and function must have rustdoc with examples
- A top-level crate doc explaining the security model and threat model
- A SECURITY.md with instructions for reporting vulnerabilities
- A clear explanation of what this crate does and does NOT protect against:
  - DOES protect: credentials at rest (encrypted), credentials in memory after use (zeroized), accidental logging (redacted)
  - Does NOT protect: credentials during active use by the LLM (the LLM sees the plaintext to type it into a form), memory forensics on a compromised host, side-channel attacks

### Dependencies

Target these crates from the RustCrypto project:

| Crate | Purpose |
|---|---|
| `aes-gcm` | AES-256-GCM encryption/decryption |
| `argon2` | Argon2id key derivation |
| `zeroize` | Secure memory wiping |
| `rand` | Cryptographically secure random number generation |
| `serde` + `serde_json` | Serialization (for credential structure and file format) |
| `base64` | Encoding for file format and transport |

Avoid pulling in large frameworks or crates with extensive dependency trees. The total dependency count should stay minimal and auditable.

---

## Public API Surface

The crate should expose approximately this API. Function signatures are illustrative — the implementer should refine based on Rust idioms and ergonomics.

```rust
// --- Data Types ---

pub enum CredentialType {
    UsernamePassword,
    ApiKey,
    Token,
    Cookie,
    Custom,
}

pub struct Credential {
    pub name: String,
    pub credential_type: CredentialType,
    fields: SecretMap,  // private, accessed via methods
    pub url_pattern: Option<String>,
    pub metadata: HashMap<String, String>,
}

pub struct CredentialSet {
    credentials: Vec<Credential>,
}

pub struct EncryptedCredential {
    pub version: u8,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

pub struct DecryptedCredentialGuard {
    // Cannot be cloned or serialized
    // Zeroizes on drop
    // Provides read-only access to Credential
}

// --- Core Operations ---

/// Encrypt a credential with a provided 256-bit key
pub fn encrypt(credential: &Credential, key: &[u8; 32]) -> Result<EncryptedCredential>;

/// Encrypt a credential set as a single blob
pub fn encrypt_set(credentials: &CredentialSet, key: &[u8; 32]) -> Result<EncryptedCredential>;

/// Decrypt an encrypted credential
pub fn decrypt(encrypted: &EncryptedCredential, key: &[u8; 32]) -> Result<DecryptedCredentialGuard>;

/// Decrypt an encrypted credential set
pub fn decrypt_set(encrypted: &EncryptedCredential, key: &[u8; 32]) -> Result<CredentialSet>;

// --- Key Derivation ---

pub struct DerivedKey {
    key: [u8; 32],  // zeroizes on drop
    pub salt: [u8; 16],
}

/// Derive a key from a passphrase (generates random salt)
pub fn derive_key(passphrase: &str) -> Result<DerivedKey>;

/// Derive a key from a passphrase with an existing salt (for decryption)
pub fn derive_key_with_salt(passphrase: &str, salt: &[u8; 16]) -> Result<DerivedKey>;

// --- Environment Loading ---

/// Load a single credential from environment variables
/// Clears the env vars after reading
pub fn load_from_env(name: &str, fields: &[&str]) -> Result<Credential>;

/// Auto-discover credentials from REMIX_CRED_* env vars
pub fn discover_from_env() -> Result<CredentialSet>;

// --- File Operations ---

/// Encrypt credentials and write to file
pub fn save_to_file(credentials: &CredentialSet, passphrase: &str, path: &Path) -> Result<()>;

/// Load and decrypt credentials from file
pub fn load_from_file(passphrase: &str, path: &Path) -> Result<CredentialSet>;
```

---

## Build Order and Timeline

This crate is on the critical path — remix-agent-runtime depends on it.

**Estimated effort: 1 week for a senior Rust developer.**

| Day | Milestone |
|---|---|
| Day 1 | Data types (`Credential`, `CredentialSet`, `CredentialType`), zeroize implementations, redacted Debug/Display |
| Day 2 | Encryption and decryption (AES-256-GCM), round-trip tests |
| Day 3 | Key derivation (Argon2id), `DerivedKey` type, tests |
| Day 4 | Environment loading, credential file format, file I/O |
| Day 5 | `DecryptedCredentialGuard` with TTL, fuzz tests, documentation, SECURITY.md |

After this ships, the runtime team can immediately start building with it. The crate is published to crates.io (or used as a git dependency) and imported into remix-agent-runtime's `Cargo.toml`.

---

## Threat Model

Be explicit about what this crate protects against and what it does not.

### In Scope (this crate mitigates these threats)

| Threat | Mitigation |
|---|---|
| Credentials leaked via logs | Debug/Display redaction, no logging of sensitive fields |
| Credentials persisted in plaintext on disk | Encrypted file format with AES-256-GCM |
| Credentials lingering in memory after use | Zeroize on drop for all sensitive types |
| Brute-force attacks on encrypted credential files | Argon2id key derivation with high memory cost |
| Tampering with encrypted credential data | AES-GCM authenticated encryption detects modification |
| Accidental cloning or serialization of decrypted credentials | No Clone/Serialize on DecryptedCredentialGuard |

### Out of Scope (this crate does NOT mitigate these threats)

| Threat | Why |
|---|---|
| Compromised host with memory access | If an attacker has root on the machine, they can read process memory. Zeroization reduces the window but cannot eliminate this. |
| LLM sees plaintext credentials | By design — the LLM needs to see the username/password to type it into a form. The credential is exposed during active use. |
| Key management and storage | This crate does not manage where encryption keys are stored. The platform is responsible for key management (KMS, Vault, etc.). |
| Side-channel attacks | Timing attacks, cache attacks, etc. are not addressed. Standard crypto implementations provide some resistance but this is not a hardened cryptographic enclave. |
| Credential theft via the LLM provider | If the user's LLM provider logs prompts, credentials sent to the LLM could be exposed. This is outside our control. |

This transparency is intentional. Users should know exactly what they're getting and what they're not.