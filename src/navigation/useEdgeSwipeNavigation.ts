import { useEffect } from "react";

const EDGE_ZONE = 24; // px from screen edge to start a swipe
const MIN_SWIPE_DISTANCE = 60; // px horizontal distance to trigger

/**
 * Detects swipes that start from the left or right edge of the screen
 * and calls the corresponding navigation callback.
 */
export function useEdgeSwipeNavigation(onSwipeLeft: () => void, onSwipeRight: () => void) {
    useEffect(() => {
        let startX = 0;
        let startY = 0;
        let isEdgeSwipe: "left" | "right" | null = null;

        const onTouchStart = (e: TouchEvent) => {
            const touch = e.touches[0];
            startX = touch.clientX;
            startY = touch.clientY;

            if (startX <= EDGE_ZONE) {
                isEdgeSwipe = "left";
            } else if (startX >= window.innerWidth - EDGE_ZONE) {
                isEdgeSwipe = "right";
            } else {
                isEdgeSwipe = null;
            }
        };

        const onTouchEnd = (e: TouchEvent) => {
            if (!isEdgeSwipe) return;

            const touch = e.changedTouches[0];
            const dx = touch.clientX - startX;
            const dy = Math.abs(touch.clientY - startY);

            // Ignore if vertical movement dominates (scrolling)
            if (dy > Math.abs(dx)) {
                isEdgeSwipe = null;
                return;
            }

            if (isEdgeSwipe === "left" && dx > MIN_SWIPE_DISTANCE) {
                onSwipeRight(); // swipe from left edge → go to previous
            } else if (isEdgeSwipe === "right" && dx < -MIN_SWIPE_DISTANCE) {
                onSwipeLeft(); // swipe from right edge → go to next
            }

            isEdgeSwipe = null;
        };

        window.addEventListener("touchstart", onTouchStart, { passive: true });
        window.addEventListener("touchend", onTouchEnd, { passive: true });
        return () => {
            window.removeEventListener("touchstart", onTouchStart);
            window.removeEventListener("touchend", onTouchEnd);
        };
    }, [onSwipeLeft, onSwipeRight]);
}
