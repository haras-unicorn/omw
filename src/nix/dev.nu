def "main" [] {
  dev -h
}

def "main test" [] {
  cd (flake-root)
  omw test lib examples
  omw test units
  omw test brain examples
}

def "main test fast" [] {
  cd (flake-root)
  with-env {
    OMW_TEST_WASM_ENGINE_NON_NATIVE: "0"
    OMW_TEST_WASM_RUNTIME_NON_NATIVE: "0"
    OMW_TEST_OPENAI_LLAMACPP: "0"
    OMW_TEST_MCP_EVERYTHING: "0"
  } {
    omw test lib examples
    omw test units
    omw test brain examples
  }
}

def "main test nixos" [test: string] {
  cd (flake-root)
  (nix build
    $".#checks.(omw system).($test)")
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
  cargo run --quiet -p omw-test -- run $dir
}

def "main format" [] {
  cd (flake-root)
  for crate in (
    (ls ./src/lib | get name)
    ++ (ls ./src/wasm | get name)
  ) {
    mkdir $"($crate)/wit"
    cp -f ./assets/omw.wit $"($crate)/wit"
  }
  for case in (omw brain example cases) {
    for spec in (omw brain example variants) {
      let dir = ($case | path join $spec.variant)
      if (($dir | path join $spec.source) | path exists) {
        (omw generate test config $case $spec.variant)
        | save -f ($dir | path join "omw.test.toml")
      }
    }
  }
  open --raw (nix build --no-link --print-out-paths ".#options")
    | prettier --parser markdown
    | save -f "./docs/deployment/nixos/options.md"
  cargo run -p omw-cli --all-features -- schema --output /dev/stdout
    | prettier --parser json
    | save -f "./assets/schema.json"
  prettier --write .
  nixfmt ...(fd '.*\.nix$' . | lines)
  cargo fmt --all
  cargo clippy --all-features --fix --allow-dirty
}

def "main lint" [] {
  cd (flake-root)
  for crate in (
    (ls ./src/lib | get name)
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
    != (cargo run -p omw-cli --all-features -- schema --output /dev/stdout
    | prettier --parser json
    | str trim)) {
    print -e "schema.json doesn't match generated"
    exit 1
  }
  for case in (omw brain example cases) {
    for spec in (omw brain example variants) {
      let dir = ($case | path join $spec.variant)
      let config = ($dir | path join "omw.test.toml")
      if (($dir | path join $spec.source) | path exists) {
        let expected = (omw generate test config $case $spec.variant)
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
  omw test lib examples
  omw test units
  omw test brain examples
  nix flake check --all-systems --show-trace
}

def "main update" [] {
  cd (flake-root)
  nix flake update
  cargo update
}

def "main release-pr" [] {
  cd (flake-root)
  omw setup git credentials
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
  (release-plz release
    --git-token $env.GITHUB_TOKEN
    --forge github
    --token $env.CARGO_REGISTRY_TOKEN
    -o json)
}

def "main build" [] {
  cd (flake-root)
  mkdir result
  (gh release upload $env.GITHUB_REF_NAME
    (omw make tarball omw)
    (omw make tarball omw rhai)
    (omw make tarball omw js)
    (omw make tarball omw-test)
    (omw make tarball omw-test rhai)
    (omw make tarball omw-test js)
    --clobber)
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
    ($env.OMW_TEST_WASM_RUNTIME_NON_NATIVE? | default "0") != "0"
  )
  let exclude = if $wasm { [] } else { [--exclude "**/wasm"] }
  if $wasm {
    cargo run --quiet -p omw-test --features compile-wasm -- compile-wasm examples
  }
  print "brain examples"
  cargo run --quiet -p omw-test -- run examples ...($exclude)
}

def "omw generate test config" [case: path, variant: string] {
  let spec = (omw brain example variants | where variant == $variant | first)
  open --raw ($case | path join "omw.test.template.toml")
    | str replace --all "{{RUNTIME}}" $spec.variant
    | str replace --all "{{SCRIPT}}" $spec.script
}

def "omw brain example cases" [] {
  glob ([ (flake-root) "examples" "**" "omw.test.template.toml" ] | path join )
    | path dirname
}

def "omw brain example variants" [] {
  [
    { variant: "rhai", source: "brain.rhai", script: "brain.rhai" }
    { variant: "js", source: "brain.js", script: "brain.js" }
    { variant: "wasm", source: "brain.rs", script: "brain.wasm" }
  ]
}

def "omw make tarball" [binary: string variant?: string] {
  let package = if $variant == null {
    $"($binary)-tarball"
  } else {
    $"($binary)-($variant)-tarball"
  }
  let suffix = if $variant == null { "" } else { $"-($variant)" }
  let build = (nix build
    --no-link
    --print-out-paths
    --show-trace
    $".#($package)") | str trim
  let name = ("result/"
    + $binary
    + $suffix
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

def "omw system" [] {
  $"(uname | get machine)-linux"
}
