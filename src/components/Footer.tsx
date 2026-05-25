import { Logo } from "./Logo";

export function Footer() {
  return (
    <footer className="border-t border-neutral-800/60 mt-20">
      <div className="max-w-6xl mx-auto px-6 py-12">
        <div className="grid md:grid-cols-4 gap-8 mb-10">
          <div className="md:col-span-2">
            <div className="flex items-center gap-2.5 mb-4">
              <Logo className="w-6 h-6 text-emerald-400" />
              <span className="font-mono font-semibold text-neutral-100">
                herdr<span className="text-emerald-400">-mcp</span>
              </span>
            </div>
            <p className="text-sm text-neutral-400 leading-relaxed max-w-sm">
              A Rust MCP server that exposes the herdr terminal multiplexer as tools.
              Built with the official <code className="font-mono text-emerald-300">rmcp</code> SDK.
            </p>
          </div>

          <div>
            <h4 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-3">Project</h4>
            <ul className="space-y-2 text-sm">
              <li><a href="#/" className="text-neutral-400 hover:text-neutral-100">Home</a></li>
              <li><a href="#/docs" className="text-neutral-400 hover:text-neutral-100">Documentation</a></li>
              <li><a href="#/playground" className="text-neutral-400 hover:text-neutral-100">Playground</a></li>
              <li><a href="#/variables" className="text-neutral-400 hover:text-neutral-100">Variables</a></li>
              <li><a href="https://github.com" target="_blank" rel="noopener" className="text-neutral-400 hover:text-neutral-100">Source</a></li>
            </ul>
          </div>

          <div>
            <h4 className="text-xs font-mono uppercase tracking-wider text-neutral-500 mb-3">Dependencies</h4>
            <ul className="space-y-2 text-sm">
              <li><a href="https://herdr.dev" target="_blank" rel="noopener" className="text-neutral-400 hover:text-neutral-100">herdr CLI ↗</a></li>
              <li><a href="https://crates.io/crates/rmcp" target="_blank" rel="noopener" className="text-neutral-400 hover:text-neutral-100">rmcp 1.7 ↗</a></li>
              <li><a href="https://modelcontextprotocol.io" target="_blank" rel="noopener" className="text-neutral-400 hover:text-neutral-100">MCP spec ↗</a></li>
              <li><a href="https://www.rust-lang.org" target="_blank" rel="noopener" className="text-neutral-400 hover:text-neutral-100">Rust ↗</a></li>
            </ul>
          </div>
        </div>

        <div className="pt-8 border-t border-neutral-800/60 flex flex-col sm:flex-row justify-between items-start sm:items-center gap-3">
          <div className="text-xs text-neutral-500 font-mono">
            herdr-mcp · AGPL-3.0 · built with 🦀
          </div>
          <div className="text-xs text-neutral-600 font-mono">
            stdio · JSON-RPC 2.0 · MCP 2025-11-25
          </div>
        </div>
      </div>
    </footer>
  );
}
