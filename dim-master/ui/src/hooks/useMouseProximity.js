import { useEffect, useState } from "react";

/**
 * Returns true when the mouse is within `threshold` pixels of the bounding
 * box of the element referenced by `ref`. Switches off after `hideDelay` ms
 * of the cursor being out of range, so the UI doesn't flicker on grazing
 * mouse movements.
 */
export function useMouseProximity(ref, { threshold = 200, hideDelay = 500 } = {}) {
  const [isNear, setIsNear] = useState(false);

  useEffect(() => {
    let hideTimer = null;

    const handleMove = (e) => {
      if (!ref.current) return;
      const rect = ref.current.getBoundingClientRect();
      const dx = Math.max(rect.left - e.clientX, 0, e.clientX - rect.right);
      const dy = Math.max(rect.top - e.clientY, 0, e.clientY - rect.bottom);
      const distance = Math.hypot(dx, dy);

      if (distance < threshold) {
        if (hideTimer) {
          clearTimeout(hideTimer);
          hideTimer = null;
        }
        setIsNear(true);
      } else if (!hideTimer) {
        hideTimer = setTimeout(() => {
          setIsNear(false);
          hideTimer = null;
        }, hideDelay);
      }
    };

    document.addEventListener("mousemove", handleMove);
    return () => {
      document.removeEventListener("mousemove", handleMove);
      if (hideTimer) clearTimeout(hideTimer);
    };
  }, [ref, threshold, hideDelay]);

  return isNear;
}
