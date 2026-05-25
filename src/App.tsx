import { HashRouter, Routes, Route, NavLink } from "react-router-dom";
import { Logo } from "./components/Logo";
import { Hero } from "./components/Hero";
import { Architecture } from "./components/Architecture";
import { Tools } from "./components/Tools";
import { Install } from "./components/Install";
import { Footer } from "./components/Footer";
import { Documentation } from "./components/Documentation";
import { Playground } from "./components/Playground";
import { VariablesPage } from "./components/VariablesPage";
import { VariableProvider } from "./components/VariableStore";

export default function App() {
  return (
    <HashRouter>
      <div className="min-h-screen bg-neutral-950 text-neutral-200 font-sans selection:bg-emerald-500/30">
        <div
          className="fixed inset-0 -z-10 opacity-[0.15]"
          style={{
            backgroundImage:
              "linear-gradient(to right, rgb(38 38 38) 1px, transparent 1px), linear-gradient(to bottom, rgb(38 38 38) 1px, transparent 1px)",
            backgroundSize: "48px 48px",
          }}
        />
        <div className="fixed inset-0 -z-10 bg-[radial-gradient(ellipse_at_top,rgba(16,185,129,0.12),transparent_50%)]" />

        <VariableProvider>
          <Header />
          <main>
            <Routes>
              <Route path="/" element={<><Hero /><Architecture /><Tools /><Install /></>} />
              <Route path="/docs" element={<Documentation />} />
              <Route path="/playground" element={<Playground />} />
              <Route path="/variables" element={<VariablesPage />} />
            </Routes>
          </main>
          <Footer />
        </VariableProvider>
      </div>
    </HashRouter>
  );
}

function Header() {
  const link = (to: string, label: string) => (
    <NavLink
      to={to}
      end={to === "/"}
      className={({ isActive }) =>
        `text-sm transition-colors ${isActive ? "text-neutral-100" : "text-neutral-400 hover:text-neutral-100"}`
      }
    >
      {label}
    </NavLink>
  );

  return (
    <header className="sticky top-0 z-40 backdrop-blur-md bg-neutral-950/70 border-b border-neutral-800/60">
      <div className="max-w-6xl mx-auto px-6 py-4 flex items-center justify-between">
        <NavLink to="/" className="flex items-center gap-2.5 group">
          <Logo className="w-7 h-7 text-emerald-400 group-hover:text-emerald-300 transition-colors" />
          <span className="font-mono font-semibold text-neutral-100 tracking-tight">
            herdr<span className="text-emerald-400">-mcp</span>
          </span>
        </NavLink>
        <nav className="hidden md:flex items-center gap-7 text-sm">
          {link("/", "Home")}
          {link("/docs", "Docs")}
          {link("/playground", "Playground")}
          {link("/variables", "Variables")}
        </nav>
        <a
          href="https://github.com"
          target="_blank"
          rel="noopener"
          className="flex items-center gap-2 text-sm font-medium px-3 py-1.5 rounded-md border border-neutral-700 bg-neutral-900/60 hover:bg-neutral-800 hover:border-neutral-600 transition-colors"
        >
          <svg className="w-4 h-4" viewBox="0 0 24 24" fill="currentColor">
            <path d="M12 .5C5.73.5.5 5.73.5 12c0 5.08 3.29 9.39 7.86 10.91.58.1.79-.25.79-.56v-2c-3.2.7-3.87-1.37-3.87-1.37-.52-1.32-1.28-1.67-1.28-1.67-1.05-.71.08-.7.08-.7 1.16.08 1.77 1.19 1.77 1.19 1.03 1.77 2.7 1.26 3.36.96.1-.75.4-1.26.73-1.55-2.55-.29-5.24-1.28-5.24-5.69 0-1.26.45-2.29 1.19-3.1-.12-.29-.52-1.47.11-3.06 0 0 .97-.31 3.18 1.18a11.03 11.03 0 0 1 5.79 0c2.21-1.49 3.18-1.18 3.18-1.18.63 1.59.23 2.77.11 3.06.74.81 1.19 1.84 1.19 3.1 0 4.42-2.69 5.39-5.25 5.68.41.36.78 1.06.78 2.14v3.17c0 .31.21.67.8.56C20.21 21.39 23.5 17.08 23.5 12 23.5 5.73 18.27.5 12 .5Z" />
          </svg>
          GitHub
        </a>
      </div>
    </header>
  );
}
