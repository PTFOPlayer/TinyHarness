# Language Detection

TinyHarness auto-detects your project's language and build tool when it starts. This information is injected into the system prompt so the AI knows how to build, test, and work with your project without being told.

## How It Works

On startup, `WorkspaceContext::collect()` scans the project root for marker files. Each language has one or more signature files — if any match, that language is detected.

### Detection Algorithm

1. Scan for primary markers (Cargo.toml, package.json, go.mod, etc.)
2. Glob-scan for secondary markers (*.csproj, *.sln, *.cabal)
3. If nothing matches, check for Makefile or Justfile as fallback hints
4. Multiple matches → monorepo detection (e.g. "Rust + Node.js")

## Supported Languages

| Language | Marker Files | Build Command | Test Command |
|----------|-------------|---------------|--------------|
| Rust | `Cargo.toml` | `cargo build` | `cargo test` |
| Zig | `build.zig`, `build.zig.zon` | `zig build` | `zig build test` |
| Deno | `deno.json`, `deno.jsonc` | `deno task build` | `deno test` |
| Bun | `bun.lockb`, `bun.lock` | `bun run build` | `bun test` |
| Swift | `Package.swift` | `swift build` | `swift test` |
| Ruby | `Gemfile` | `bundle install` | `bundle exec rspec` |
| Elixir | `mix.exs` | `mix compile` | `mix test` |
| Haskell | `stack.yaml`, `*.cabal` | `stack build` | `stack test` |
| Kotlin | `build.gradle.kts`, `settings.gradle.kts` | `gradle build` | `gradle test` |
| .NET | `*.csproj`, `*.sln` | `dotnet build` | `dotnet test` |
| Dart/Flutter | `pubspec.yaml` | `dart run build` | `dart test` |
| Nix | `flake.nix`, `default.nix` | `nix build` | `nix flake check` |
| Node.js | `package.json` | `npm run build` | `npm test` |
| Python | `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt` | `pip install -e .` | `pytest` |
| Go | `go.mod` | `go build ./...` | `go test ./...` |
| Java (Gradle) | `build.gradle` | `gradle build` | `gradle test` |
| Java (Maven) | `pom.xml` | `mvn compile` | `mvn test` |
| C/C++ (CMake) | `CMakeLists.txt` | `cmake --build build` | `ctest` |

## Fallback Detection

When no language marker is found, TinyHarness checks for:

| File | Detection | Build | Test |
|------|-----------|-------|------|
| `Makefile` | "Unknown (has Makefile)" | `make` | `make test` |
| `Justfile` | "Unknown (has Justfile)" | `just build` | `just test` |

## Monorepo Detection

When multiple language markers are found in the same directory, TinyHarness joins them:

```
Cargo.toml + package.json → "Rust + Node.js"
```

The build and test commands come from the first detected language.

## Viewing Detected Context

Use `/context` to see what TinyHarness detected:

```
Workspace Context:
  Project: TinyHarness (Rust)
  Root: /home/user/projects/TinyHarness
  Git repo: yes
  Build: cargo build
  Test: cargo test

Structure:
  src/  (agent/, commands/)
  tinyharness-lib/  (src/, Cargo.toml, prompts/)
  ...
```

The detected type also appears in the system prompt:

```
You are operating inside a Rust project called "TinyHarness".
```

## What the AI Sees

The detection results are injected into the system prompt at startup:

```
You are operating inside a Rust project called "TinyHarness".
Workspace root: /home/user/projects/TinyHarness
This is a git repository.

Project structure:
  src/
  tinyharness-lib/
  ...

Build command: cargo build
Test command: cargo test
```

This means the AI knows how to build and test your project from the first message — no need to explain it.
