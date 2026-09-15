# openapi-modelgen: Agent Instructions

A code generator: it reads an OpenAPI 3.0 spec and writes a Rust crate of request and response
types, with serde derives, `Validation` impls, and default functions.

## Repo Structure

```
src/parse/          # OpenAPI -> Entity (structs, enums, unions), records diagnostics
  mod.rs            #   entry point, the Parser collector, $ref helpers
  schema.rs         #   components/schemas -> structs and enums, the type map
  one_of.rs         #   top-level oneOf -> unions, and the pass that prunes them
  operation.rs      #   everything under paths: query parameters, operation bodies
  default.rs        #   whether a field's `default` can be emitted, and as what
  constraint.rs     #   validation keywords -> Constraints
  testutil.rs       #   spec fixtures shared by the parser's test modules
src/write.rs        # Entity -> generated crate (model.rs, validation.rs, default.rs, Cargo.toml)
src/ident.rs        # Spec strings -> Rust identifiers, the single place that names anything
src/diagnostic.rs   # Diagnostic and Severity: what the generator could not represent
src/error.rs        # Error type, including the fatal-diagnostic abort
src/lib.rs          # Public API (parse, write, generate), domain types, integration tests
src/main.rs         # CLI (clap)
tests/              # CLI behavior: --strict, naming aborts, files written or not
examples/           # Example crate, its openapi.yaml, and the generated output (committed)
justfile            # All common tasks, read this first
```

No identifier is a spec string. Every name is derived through `ident`, and a name composed from
that output (the struct-prefixed inline enum, a default function) passes `ident::assert_ident`
before it is emitted, so a gap in the sanitizers fails loudly instead of writing broken code.

## Dev Environment

Requires Nix with flakes. Cargo is not on the PATH outside the dev shell.

```bash
nix develop             # Enter the dev shell (Rust toolchain, clippy, rustfmt, just)
nix develop -c just ci  # Or run a single command in it
```

## Common Tasks

```bash
just ci        # Full pipeline: build (incl. the example), fmt check, clippy -D warnings, test
just build     # Build the generator and the example crate
just test      # cargo test
just fmt       # cargo fmt --all
just lint      # cargo clippy -- -D warnings
just run ARGS  # cargo run -- ARGS
```

Run `just ci` before every commit. GitHub Actions runs the same recipe.

## Verifying a Change

Generated code that compiles is the point, so a test suite that passes is not enough on its own.
For a change that affects output:

1. Generate from a spec that exercises it, then build the result. It must compile with no
   warnings: rustc lints the generated crate, so a field that is not snake_case or a type that is
   not camel case is a defect in the generator.
2. Regenerate `examples/axum-with-custom-extractors/hello-world-openapi` and check the diff. That
   crate is committed, so a change there is a change to what every user gets.

## Diagnostics

Nothing is dropped or degraded silently. Each case records a `Diagnostic`:

- `Dropped`: the construct produced no output.
- `Degraded`: the construct produced a lossy fallback.
- `Fatal`: the construct cannot be generated at all, and no fallback would be honest. Two spec
  names that map to one Rust name, a name with nothing to build one from, a `$ref` that resolves
  to no type. `generate` returns an error, and the CLI writes no files.

Prefer a fatal diagnostic over inventing a name. A name that appears nowhere in the spec is worse
than a failed run, because the user cannot find it.

## Writing Conventions

Applies to everything written by hand: code comments, doc comments, Markdown, commit messages,
diagnostic and error strings, replies to a prompt.

- **Use as few words as possible.** Pick every word meticulously to reduce the volume to a strict
  minimum. Be down to the point. Less is more.
- **Avoid superlatives and praise.** Do not tell the reader they are absolutely right. Give the
  cold hard truth.
- **Always use ASD-STE100 Simplified Technical English.** One idea per sentence, active voice,
  present tense, articles kept in place, and one meaning per word. Prefer the approved word:
  `use` over `utilize`, `start` over `initiate`, `remove` over `eliminate`.
- **No em dashes (`—`) or en dashes (`–`).** Use a colon, a comma, brackets, or two sentences.
  They are hard to type, easy to confuse with a hyphen, and a reliable tell that text was written
  by an LLM. A hyphen in `well-known` or in `--flag` is a hyphen and is fine.
- **No single-character ellipsis (`…`).** Write three dots: `...`. Same reasons, hard to type and
  an immediate giveaway that the sentence was not typed by a person.
- **American spelling**: `behavior`, `color`, `serialize`.
- **A doc comment leads with what the item does**, in one sentence, and puts the details in the
  paragraphs after it. A first line that opens on a detail, or that is a noun phrase with no verb,
  makes the reader assemble the purpose themselves.
- **A comment says why, not what.** The code states what it does. Record the constraint that made
  it that way, the case that broke, or the alternative that does not work.
- **Strings the tool prints follow the same rules**, and they are also part of the output the user
  reads under pressure. Name the spec construct, quote the offending value, and say what to do.
- Generated files are exempt, since their text comes from the generator, but the templates in
  `src/write.rs` that produce that text are not.

## Testing

- Unit tests live next to the code in `src/*.rs`, integration tests in `src/lib.rs` (they drive
  `generate` end to end) and in `tests/` (they drive the CLI binary).
- Assert against whole generated blocks, not single lines. One raw string holding a struct or an
  enum reads as "this is the code we generate" and catches field order and attributes too. Tests
  written before this rule still assert line by line; convert them when you touch them.
- A test for a naming or diagnostic rule states the spec input, the generated output, and the
  diagnostics. A rule with no diagnostic assertion is half tested.

## Git Conventions

- **Run `just ci` before committing.** Unformatted code and clippy warnings are the usual cause of
  a red pipeline.
- The main branch is called `main`.
- Rebase onto main, no merge commits.
- Follow [cbea.ms commit message style](https://cbea.ms/git-commit/).
- **Keep commit messages brief**: a subject line, plus a short paragraph on the why where the diff
  does not show it. Do not restate the diff, enumerate the files touched, or run to several
  paragraphs; that buries the one sentence worth reading. What a future reader needs while looking
  at the code belongs in a comment there, not in the log.
- Squash fixup commits before merging.
