export function CropMarks() {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-3 z-20 text-ink/40">
      <span className="absolute left-0 top-0 h-4 w-4 border-l border-t border-current" />
      <span className="absolute right-0 top-0 h-4 w-4 border-r border-t border-current" />
      <span className="absolute bottom-0 left-0 h-4 w-4 border-b border-l border-current" />
      <span className="absolute bottom-0 right-0 h-4 w-4 border-b border-r border-current" />
    </div>
  );
}
