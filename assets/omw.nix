{
  services.omw = {
    enable = true;
    user = "omw";
    group = "omw";
    stateDir = "omw";
    settings.providers.openai = {
      kind = "openai";
      api_key = "$OPENAI_API_KEY";
      model = "gpt-4o";
    };
    settings.tooling.fs = {
      kind = "mcp";
      command = "npx";
      args = [
        "-y"
        "@modelcontextprotocol/server-filesystem"
        "/var/lib/omw"
      ];
    };
    settings.runtime.rhai.kind = "rhai";
    settings.agents = [
      {
        name = "alice";
        runtime = "rhai";
        script = "/var/lib/omw/brain.rhai";
      }
    ];
    environment.OPENAI_API_KEY = "…";
  };
}
