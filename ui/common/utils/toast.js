export function showToast(message) {
  const toast = document.createElement("div");
  toast.setAttribute("popover", "manual");
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
    inset: "auto",
    margin: "0",
    opacity: "0",
    transition: "opacity 200ms",
  });
  document.body.appendChild(toast);
  toast.showPopover();
  setTimeout(() => { toast.style.opacity = "1"; }, 10);
  setTimeout(() => {
    toast.style.opacity = "0";
    setTimeout(() => { toast.hidePopover(); toast.remove(); }, 200);
  }, 2000);
}
