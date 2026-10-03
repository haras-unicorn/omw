def "main" [] {
  dev -h
}

def "main test" [] {
  cd (flake-root)
  omw test units
  omw test lib examples
  omw test brain examples
}

def "main test fast" [] {
  cd (flake-root)
  with-env {
    OMW_TEST_WASM_ENGINE_NON_NATIVE: "0"
    OMW_TEST_WASM_RUNTIME_NON_NATIVE: "0"
    OMW_TEST_OPENAI_LLAMACPP: "0"
    OMW_TEST_MCP_EVERYTHING: "0"
    OMW_TEST_EXAMPLE_WASM: "0"
  } {
    omw test units
    omw test lib examples
    omw test brain examples
  }
}

def "main test nixos" [test: string] {
  cd (flake-root)
  (nix build -L $".#checks.(omw system).($test)")
}

def "main test example lib" [example: string] {
  cd (flake-root)
  cargo run --all-features -p omw --example $example
}

def "main test example brain" [case: string, variant: string] {
  cd (flake-root)
  let dir = $"examples/($case)/($variant)"
  if $variant == "wasm" {
    cargo run --quiet -p omw-test --features compile-wasm -- compile-wasm $dir
  }
  cargo run --quiet -p omw-test -- run $"./examples/($case)" --include $"($variant)/**"
}

def "main test test tty" [] {
  cd (flake-root)
  cargo run --quiet -p omw-test -- run ./examples --exclude "**/wasm/**"
}

def "main test cli tty" [] {
  cd (flake-root)
  cargo run --quiet -p omw-cli --features runtime-js -- run --config ./assets/omw.tty.toml
}

def "main test cli tenere" [] {
  cd (flake-root)
  ('llm = "chatgpt"'
    + "\n"
    + "\n" + '[chatgpt]'
    + "\n" + 'openai_api_key = "omw"'
    + "\n" + 'model = "model"'
    + "\n" + 'url = "http://127.0.0.1:37532/v1/chat/completions"')
    | tenere -c /dev/stdin
}

def "main format" [] {
  cd (flake-root)
  for crate in (
    (ls ./src/lib | get name | where $it != "omw-output")
    ++ (ls ./src/wasm | get name)
  ) {
    mkdir $"($crate)/wit"
    cp -f ./assets/omw.wit $"($crate)/wit"
  }
  let variants = (omw brain example variants)
  for case in (omw brain example cases) {
    for spec in $variants {
      let dir = ($case | path join $spec.variant)
      if (($dir | path join $spec.source) | path exists) {
        (omw generate test config $case $spec)
          | save -f ($dir | path join "omw.test.toml")
      }
    }
  }
  open --raw (nix build --no-link --print-out-paths ".#options")
    | prettier --parser markdown
    | save -f "./docs/deployment/nixos/options.md"
  cargo run -p omw-cli -- schema --output /dev/stdout
    | prettier --parser json
    | save -f "./assets/schema.json"
  cargo run -p omw-test -- schema --output /dev/stdout
    | prettier --parser json
    | save -f "./assets/schema.test.json"
  (omw config types ./assets/schema.json OmwConfig)
    | prettier --parser typescript
    | save -f "./assets/schema.d.ts"
  (omw config types ./assets/schema.test.json OmwTestConfig)
    | prettier --parser typescript
    | save -f "./assets/schema.test.d.ts"
  (omw all types)
    | prettier --parser typescript
    | save -f "./src/wasm/omw-wasm-js-interpreter/omw.all.d.ts"
  prettier --write .
  nixfmt ...(fd '.*\.nix$' . | lines)
  cargo fmt --all
  cargo clippy --all-features --fix --allow-dirty
}

def "main lint" [] {
  cd (flake-root)
  dev lint check
  dev lint test
  dev lint nix --all-systems
  dev lint build
}

def "main lint check" [] {
  cd (flake-root)
  for crate in (
    (ls ./src/lib | get name | where $it != "omw-output")
    ++ (ls ./src/wasm | get name)
  ) {
    if ((open --raw ./assets/omw.wit)
      != (open --raw $"($crate)/wit/omw.wit")) {
      print -e $"($crate)/wit/omw.wit does not match assets/omw.wit"
      exit 1
    }
  }
  if ((open --raw ./docs/deployment/nixos/options.md
    | str trim)
    != (open --raw (nix build --no-link --print-out-paths ".#options")
    | prettier --parser markdown
    | str trim)) {
    print -e "options.md doesn't match generated"
    exit 1
  }
  if ((open --raw ./assets/schema.json
    | str trim)
    != (cargo run -p omw-cli -- schema --output /dev/stdout
    | prettier --parser json
    | str trim)) {
    print -e "schema.json doesn't match generated"
    exit 1
  }
  if ((open --raw ./assets/schema.test.json
    | str trim)
    != (cargo run -p omw-test -- schema --output /dev/stdout
    | prettier --parser json
    | str trim)) {
    print -e "schema.test.json doesn't match generated"
    exit 1
  }
  if ((open --raw ./assets/schema.d.ts
    | str trim)
    != (omw config types ./assets/schema.json OmwConfig
    | prettier --parser typescript
    | str trim)) {
    print -e "schema.d.ts doesn't match generated"
    exit 1
  }
  if ((open --raw ./assets/schema.test.d.ts
    | str trim)
    != (omw config types ./assets/schema.test.json OmwTestConfig
    | prettier --parser typescript
    | str trim)) {
    print -e "schema.test.d.ts doesn't match generated"
    exit 1
  }
  if ((open --raw ./src/wasm/omw-wasm-js-interpreter/omw.all.d.ts
    | str trim)
    != (omw all types
    | prettier --parser typescript
    | str trim)) {
    print -e "omw.all.d.ts doesn't match generated"
    exit 1
  }
  let variants = (omw brain example variants)
  for case in (omw brain example cases) {
    for spec in $variants {
      let dir = ($case | path join $spec.variant)
      let config = ($dir | path join "omw.test.toml")
      if (($dir | path join $spec.source) | path exists) {
        let expected = (omw generate test config $case $spec)
        if (
          ($expected | str trim)
          != (open --raw $config | str trim)
        ) {
          print -e $"($config) does not match its template"
          exit 1
        }
      }
    }
  }
  prettier --check .
  cspell lint . --no-progress
  nixfmt --check ...(fd '.*\.nix$' . | lines)
  markdownlint --ignore-path .markdownignore .
  if ($env.NIX_BUILD_TOP? | is-empty) {
    (markdown-link-check
      --config .markdown-link-check.json
      --quiet
      ...(fd '.*.md' . | lines))
    (taplo lint
      --schema ("https://raw.githubusercontent.com"
        + "/release-plz/release-plz"
        + "/refs/tags/release-plz-v0.3.148/.schema/latest.json")
      .release-plz.toml)
  }
  cargo fmt --all -- --check
}

def "main lint test" [] {
  cd (flake-root)
  omw test units
  omw test lib examples
  omw test brain examples
}

def "main lint nix" [--all-systems] {
  cd (flake-root)
  let args = if $all_systems { [ "--all-systems" ] } else { [ ] }
  nix flake check --show-trace ...($args)
}

def "main lint build" [] {
  cd (flake-root)
  nix build --show-trace ".#omw-tarball"
}

def "main update" [] {
  cd (flake-root)
  nix flake update
  cargo update
}

def "main release-pr" [] {
  cd (flake-root)
  omw setup git credentials
  rm -rf .cargo
  let repo = $"($env.GITHUB_SERVER_URL)/($env.GITHUB_REPOSITORY)"
  (release-plz release-pr
    --git-token $env.GITHUB_TOKEN
    --repo-url $repo
    --forge github
    -o json)
}

def "main release" [] {
  cd (flake-root)
  omw setup git credentials
  rm -rf ./src/lib/omw/wasm
  touch ./src/lib/omw/build.rs
  with-env { OMW_WASM_BUILD_VENDORED: "1" } {
    cargo build --release -p omw --features runtime-rhai,runtime-js,mock
  }
  let dir = "./src/lib/omw/wasm"
  for guest in (omw guests) {
    let file = ($dir | path join $"($guest).component.wasm")
    if not ($file | path exists) {
      print -e $"prebuild did not produce ($file)"
      exit 1
    }
  }
  rm -rf .cargo
  (release-plz release
    --git-token $env.GITHUB_TOKEN
    --forge github
    --token $env.CARGO_REGISTRY_TOKEN
    -o json)
}

def "main build" [] {
  cd (flake-root)
  mkdir result
  let matrix = (omw matrix)
  for row in ($matrix | where {|row| $row.format == "tarball" }) {
    let name = (omw make tarball $row.program $row.variant_suffix)
    (gh release upload $env.GITHUB_REF_NAME $name --clobber)
  }
  for row in (
    $matrix | where {|row|
      ($row.format == "unwrapped") or ($row.format == "wrapped")
    }
  ) {
    let path = (nix build
      --no-link
      --print-out-paths
      --show-trace
      $".#($row.attr)") | str trim
    (cachix push haras-releases $path)
    (cachix pin haras-releases
      $"($row.attr)-(omw system)"
      $path
      --keep-days 365)
  }
}

def "omw test units" [] {
  cargo clippy --all-features -- -D warnings
  cargo test --all-features
}

def "omw test lib examples" [] {
  for file in (
    ls ./src/lib/omw/examples | where name ends-with ".rs" | get name
  ) {
    let stem = ($file | path parse | get stem)
    print $"lib example: ($stem)"
    cargo run --all-features -p omw --example $stem
  }
}

def "omw test brain examples" [] {
  cd (flake-root)
  let wasm = (
    ($env.OMW_TEST_EXAMPLE_WASM? | default "0") != "0"
  )
  let exclude = if $wasm { [] } else { [--exclude "**/wasm/**"] }
  if $wasm {
    cargo run --quiet -p omw-test --features compile-wasm -- compile-wasm ./examples
  }
  print "brain examples"
  cargo run --quiet -p omw-test -- run ./examples ...($exclude)
}

def "omw generate test config" [case: path, spec: any] {
  let base = (open ($case | path join "omw.test.base.toml"))
  let agents = ($base | get agents | columns | sort)
  mut out = (
    "# Generated by `dev format` from omw.test.base.toml.\n"
    + "# This file only carries what differs per variant: the runtime kind and\n"
    + "# each agent's brain script. The base config supplies the rest.\n\n"
    + "[runtime.runtime]\n"
    + $"kind = \"($spec.variant)\"\n"
  )
  for name in $agents {
    $out = $out + $"\n[agents.($name)]\nscript = \"($spec.script)\"\n"
  }
  $out
}

def "omw config types" [schema: path, namespace: string] {
  let raw = (json2ts -i $schema --unreachableDefinitions)
  "declare namespace " + $namespace + " {\n" + $raw + "\n}\n"
}

def "omw make tarball" [program: string, variant_suffix: string] {
  let package = $"($program)($variant_suffix)-tarball"
  let build = (nix build
    --no-link
    --print-out-paths
    --show-trace
    $".#($package)") | str trim
  let name = ("result/"
    + $program
    + $variant_suffix
    + $"-(omw system)"
    + ".tar.gz")
  ln -sf $build $name
  return $name
}

def "omw setup git credentials" [] {
  let json = (gh api graphql
    -f query='query { viewer { name login databaseId } }'
    --jq '.data.viewer')
  let name = $json | jq --raw-output '.name // .login'
  let email = $json
    | jq --raw-output ('"\(.databaseId)+\(.login)'
        + '@users.noreply.github.com"')
  git config --global user.name $name
  git config --global user.email $email
}

def "omw brain example variants" [] {
  (
    omw variants | each {|variant|
      {
        variant: $variant.runtime
        source: $variant.source
        script: $variant.script
      }
    }
  )
}

def "omw matrix" [] {
  let binaries = (omw binaries)
  let variants = (omw variants)
  let formats = (omw formats)
  (
    $binaries | each {|binary|
      $variants | each {|variant|
        $formats | each {|format|
          {
            crate: $binary.crate
            program: $binary.program
            variant: $variant.key
            variant_suffix: $variant.suffix
            runtime: $variant.runtime
            source: $variant.source
            script: $variant.script
            format: $format.key
            format_suffix: $format.suffix
            attr: $"($binary.program)($variant.suffix)($format.suffix)"
          }
        }
      }
    } | flatten | flatten
  )
}

def "omw binaries" [] {
  omw flake lib "binaries"
}

def "omw variants" [] {
  omw flake lib "variants"
}

def "omw formats" [] {
  omw flake lib "formats"
}

def "omw flake lib" [key: string] {
  cd (flake-root)
  nix eval --json $".#lib.($key)" | from json
}

def "omw brain example cases" [] {
  glob ([ (flake-root) "examples" "**" "omw.test.base.toml" ] | path join )
    | path dirname
}

def "omw all types" [] {
  [
    (open --raw ./assets/schema.d.ts)
    (open --raw ./assets/schema.test.d.ts)
    (open --raw ./src/wasm/omw-wasm-js-interpreter/omw.d.ts)
  ] | str join "\n\n"
}

def "omw guests" [] {
  [
    omw-wasm-rhai-interpreter
    omw-wasm-js-interpreter
    omw-wasm-mock
  ]
}

def "omw system" [] {
  $"(uname | get machine)-linux"
}
