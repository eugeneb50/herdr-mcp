import { useEffect, useState } from "react";

type ConfigData = {
  http_port?: string;
  data_dir?: string;
  herdr_socket?: string;
  herdr_bin?: string;
};

export function Settings() {
  const [config, setConfig] = useState<ConfigData | null>(null);

  useEffect(() => {
    fetch("/api/config")
      .then((r) => r.json())
      .then(setConfig)
      .catch(() => { /* server may be unavailable */ });
  }, []);

  return (
    <div className="max-w-6xl mx-auto px-6 py-10 space-y-8">
      <h1 className="text-xl font-mono font-semibold text-neutral-100">Settings</h1>

      <section>
        <h2 className="text-sm font-mono uppercase tracking-wider text-neutral-500 mb-3">Runtime</h2>
        <div className="rounded-lg border border-neutral-800 divide-y divide-neutral-800">
          <Row label="HTTP Port" value={config?.http_port ?? "..."} />
          <Row label="Data Directory" value={config?.data_dir ?? "..."} />
          <Row label="herdr Socket" value={config?.herdr_socket || "(not set)"} />
          <Row label="herdr Binary" value={config?.herdr_bin ?? "..."} />
        </div>
      </section>

      <section>
        <h2 className="text-sm font-mono uppercase tracking-wider text-neutral-500 mb-3">Environment Overrides</h2>
        <div className="rounded-lg border border-neutral-800 divide-y divide-neutral-800">
          <EnvRow name="HERDR_SOCKET_PATH" value={config?.herdr_socket} />
          <EnvRow name="HERDR_MCP_DATA_DIR" value={config?.data_dir} />
          <EnvRow name="HERDR_MCP_HTTP_PORT" value={config?.http_port} />
          <EnvRow name="HERDR_BIN" value={config?.herdr_bin} />
        </div>
      </section>
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between px-4 py-3">
      <span className="text-sm text-neutral-400">{label}</span>
      <span className="text-sm font-mono text-neutral-200">{value}</span>
    </div>
  );
}

function EnvRow({ name, value }: { name: string; value?: string }) {
  const isSet = !!value;
  return (
    <div className="flex items-center justify-between px-4 py-3">
      <span className="text-sm text-neutral-400 font-mono">{name}</span>
      <span className={`text-sm font-mono ${isSet ? "text-emerald-400" : "text-neutral-600"}`}>
        {isSet ? `[set] ${value}` : "[not set]"}
      </span>
    </div>
  );
}
