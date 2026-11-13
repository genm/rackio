import { useEffect, useRef, useState } from "react";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(", ");

/**
 * Make a `role="dialog" aria-modal="true"` surface behave like a modal:
 * Escape closes it, Tab cannot leave it for the page it declares inert, and the
 * control that opened it regains focus when it closes.
 *
 * Declaring `aria-modal` without these tells assistive technology the rest of
 * the page is unavailable while still letting keyboard focus wander into it,
 * which is worse than not declaring it at all.
 */
export function useModalDialog<T extends HTMLElement>(onClose: () => void) {
  const dialogRef = useRef<T>(null);
  const closeRef = useRef(onClose);
  // Read during the first render, before the dialog is committed: by the time
  // effects run React has already applied any `autoFocus` inside the dialog, so
  // reading the active element then would "restore" focus to a node that is
  // about to be removed and leave it on <body>.
  const [opener] = useState(() => document.activeElement);

  useEffect(() => {
    closeRef.current = onClose;
  });

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog === null) return;
    const focusable = () => Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR));
    // React has already honoured any `autoFocus` by now; only place focus when
    // the dialog did not claim it, so the intended first field keeps it.
    if (!dialog.contains(document.activeElement)) {
      (focusable()[0] ?? dialog).focus();
    }

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        closeRef.current();
        return;
      }
      if (event.key !== "Tab") return;
      const items = focusable();
      const first = items[0];
      const last = items[items.length - 1];
      if (first === undefined || last === undefined) {
        event.preventDefault();
        dialog.focus();
        return;
      }
      const active = document.activeElement;
      const outside = !dialog.contains(active);
      if (event.shiftKey && (outside || active === first)) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && (outside || active === last)) {
        event.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      if (opener instanceof HTMLElement && document.contains(opener)) {
        opener.focus();
      }
    };
  }, [opener]);

  return dialogRef;
}
