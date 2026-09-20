// Toasts share one fixed column so repeat actions stack instead of piling up
// on the same spot (overlapping text reads as a stuck message).
let stack = null;

function toastStack() {
  if (stack?.isConnected) return stack;
  stack = document.createElement("div");
  Object.assign(stack.style, {
    position: "fixed",
    bottom: "2rem",
    left: "50%",
    transform: "translateX(-50%)",
    display: "flex",
    flexDirection: "column",
    alignItems: "center",
    gap: "0.5rem",
    zIndex: "10000",
    pointerEvents: "none",
  });
  document.body.appendChild(stack);
  return stack;
}

export function showToast(message) {
  const toast = document.createElement("div");
  toast.textContent = message;
  Object.assign(toast.style, {
    backgroundColor: "var(--color-bg-surface)",
    color: "var(--color-text-main)",
    padding: "0.75rem 1.5rem",
    borderRadius: "var(--radius-md)",
    boxShadow: "var(--shadow-lg)",
    border: "1px solid var(--color-border)",
    fontSize: "var(--font-size-sm)",
    maxWidth: "min(90vw, 400px)",
    textAlign: "center",
    wordBreak: "break-word",
    opacity: "0",
    transition: "opacity 200ms",
  });
  toastStack().appendChild(toast);
  requestAnimationFrame(() => { toast.style.opacity = "1"; });
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => { toast.remove(); }, 200);
  }, 3000);
}
