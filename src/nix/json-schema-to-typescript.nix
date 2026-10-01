{
  perSystem =
    { pkgs, lib, ... }:
    {
      packages.json-schema-to-typescript = pkgs.buildNpmPackage {
        pname = "json-schema-to-typescript";
        version = "16.0.0";

        src = pkgs.fetchFromGitHub {
          owner = "bcherny";
          repo = "json-schema-to-typescript";
          rev = "7f72770eb854328c96b112be445da0306bebdbaf";
          hash = "sha256-s3IZoNbCjqMJJ5LHjKgzGKUFwKphLfXyN8g0PVMY7qM=";
        };

        npmDepsHash = "sha256-SSUSq74ReueZclZ5nhUpJnynQv6hWVYCUbNhmc336Hc=";
        npmBuildScript = "build:server";

        meta = {
          description = "Compile JSON Schema to TypeScript typings";
          homepage = "https://github.com/bcherny/json-schema-to-typescript";
          license = lib.licenses.mit;
          mainProgram = "json2ts";
        };
      };
    };
}
