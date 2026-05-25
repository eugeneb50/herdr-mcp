import { Logo } from "./Logo";

export function Architecture() {
  return (
    <section id="architecture" className="relative max-w-6xl mx-auto px-6 py-20 border-t border-neutral-800/60">
      <div className="mb-12 max-w-2xl">
        <div className="text-xs font-mono text-emerald-400 uppercase tracking-wider mb-3">Architecture</div>
        <h2 className="text-3xl md:text-4xl font-bold text-neutral-50 tracking-tight mb-4">
          A thin wrapper over the stable CLI.
        </h2>
        <p className="text-neutral-400 leading-relaxed">
          herdr-mcp shells out to the local <code className="font-mono text-emerald-300">herdr</code> binary
          rather than speaking the undocumented socket protocol directly. That means zero coupling,
          and the server keeps working as herdr evolves.
        </p>
      </div>

      <div className="grid md:grid-cols-3 gap-4 mb-10">
        <Node
          label="MCP client"
          sub="Claude · Cursor · Continue"
          icon={
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
              <path strokeLinecap="round" strokeLinejoin="round" d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" />
            </svg>
          }
          accent="sky"
        />
        <Node
          label="herdr-mcp"
          sub="Rust · rmcp 1.7 · stdio"
          icon={<Logo className="w-5 h-5" />}
          accent="emerald"
          highlight
        />
        <Node
          label="herdr CLI"
          sub="terminal multiplexer"
          icon={
            <svg className="w-5 h-5" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth="1.8">
              <path strokeLinecap="round" strokeLinejoin="round" d="M8 9l3 3-3 3m5 0h3M5 20h14a2 2 0 002-2V6a2 2 0 00-2-2H5a2 2 0 00-2 2v12a2 2 0 002 2z" />
            </svg>
          }
          accent="amber"
        />
      </div>

      <div className="grid md:grid-cols-3 gap-8 text-sm">
        <Reason
          title="Stable surface"
          body="The CLI is herdr's documented, versioned contract. We don't depend on internal socket frames that may change between releases."
        />
        <Reason
          title="Zero coupling"
          body="Any machine with the `herdr` binary in PATH works. No protocol version handshake, no state to keep in sync."
        />
        <Reason
          title="Single static binary"
          body="Compiles to one Rust binary with no runtime deps. Drop it into your MCP config and you're done."
        />
      </div>
    </section>
  );
}

function Node({
  label,
  sub,
  icon,
  accent,
  highlight = false,
}: {
  label: string;
  sub: string;
  icon: React.ReactNode;
  accent: "sky" | "emerald" | "amber";
  highlight?: boolean;
}) {
  const colors = {
    sky: "text-sky-300 border-sky-500/30 bg-sky-500/5",
    emerald: "text-emerald-300 border-emerald-500/40 bg-emerald-500/10",
    amber: "text-amber-300 border-amber-500/30 bg-amber-500/5",
  }[accent];

  return (
    <div
      className={`relative flex items-center gap-3 p-4 rounded-lg border ${colors} ${
        highlight ? "ring-1 ring-emerald-400/30 shadow-lg shadow-emerald-500/5" : ""
      }`}
    >
      <div className="flex-shrink-0">{icon}</div>
      <div className="min-w-0">
        <div className="font-mono font-semibold text-neutral-100 truncate">{label}</div>
        <div className="text-xs text-neutral-400 truncate">{sub}</div>
      </div>
    </div>
  );
}

function Reason({ title, body }: { title: string; body: string }) {
  return (
    <div>
      <h3 className="text-neutral-100 font-semibold mb-2 flex items-center gap-2">
        <span className="w-1 h-4 bg-emerald-400 rounded-full" />
        {title}
      </h3>
      <p className="text-neutral-400 leading-relaxed">{body}</p>
    </div>
  );
}
