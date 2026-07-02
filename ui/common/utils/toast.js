export function showToast(message) {
  const toast = document.createElement("div");
  toast.textContent = message;
  Object.assign(toast.style, {
    position: "fixed",
    bottom: "2rem",
    left: "50%",
    transform: "translateX(-50%)",
    backgroundColor: "var(--color-bg-surface)",
    color: "var(--color-text-main)",
    padding: "0.75rem 1.5rem",
    borderRadius: "var(--radius-md)",
    boxShadow: "var(--shadow-lg)",
    border: "1px solid var(--color-border)",
    fontSize: "var(--font-size-sm)",
    zIndex: "10000",
    maxWidth: "min(90vw, 400px)",
    textAlign: "center",
    wordBreak: "break-word",
    opacity: "0",
    transition: "opacity 200ms",
  });
  document.body.appendChild(toast);
  requestAnimationFrame(() => { toast.style.opacity = "1"; });
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => { toast.remove(); }, 200);
  }, 3000);
}
