{
  services.omw = {
    enable = true;
    variant = "rhai";
    user = "omw";
    group = "omw";
    stateDir = "omw";
    settings.providers.openai = {
      kind = "openai";
      model = "gpt-4o";
    };
    settings.tooling.fs = {
      kind = "mcp";
      transport = "stdio";
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
    environment.OMW__PROVIDERS__OPENAI__API_KEY = "…";
  };
}
