export function Logo({ className = "" }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" className={className} xmlns="http://www.w3.org/2000/svg">
      <rect x="2" y="3" width="20" height="18" rx="2.5" stroke="currentColor" strokeWidth="1.6" />
      <path d="M2 7.5h20" stroke="currentColor" strokeWidth="1.6" />
      <path d="M12 7.5v13.5" stroke="currentColor" strokeWidth="1.6" />
      <circle cx="5" cy="5.5" r="0.8" fill="currentColor" />
      <circle cx="7.5" cy="5.5" r="0.8" fill="currentColor" />
      <circle cx="10" cy="5.5" r="0.8" fill="currentColor" />
      <path d="m5 12.5 2 2-2 2" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M15 16.5h3" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
    </svg>
  );
}
