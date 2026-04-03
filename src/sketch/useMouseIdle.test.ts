import { renderHook, act } from '@testing-library/react';
import { useMouseIdle } from './useMouseIdle';

describe('useMouseIdle', () => {
    beforeEach(() => { vi.useFakeTimers(); });
    afterEach(() => { vi.useRealTimers(); });

    it('starts as not idle', () => {
        const { result } = renderHook(() => useMouseIdle());
        expect(result.current).toBe(false);
    });

    it('becomes idle after 3 seconds of inactivity', () => {
        const { result } = renderHook(() => useMouseIdle());

        act(() => { vi.advanceTimersByTime(3000); });
        expect(result.current).toBe(true);
    });

    it('resets on mousemove', () => {
        const { result } = renderHook(() => useMouseIdle());

        act(() => { vi.advanceTimersByTime(2500); });
        expect(result.current).toBe(false);

        act(() => { window.dispatchEvent(new Event('mousemove')); });
        act(() => { vi.advanceTimersByTime(2500); });
        expect(result.current).toBe(false);

        act(() => { vi.advanceTimersByTime(500); });
        expect(result.current).toBe(true);
    });

    it('resets on mousedown', () => {
        const { result } = renderHook(() => useMouseIdle());

        act(() => { vi.advanceTimersByTime(2900); });
        act(() => { window.dispatchEvent(new Event('mousedown')); });
        act(() => { vi.advanceTimersByTime(2900); });
        expect(result.current).toBe(false);
    });

    it('cleans up listeners on unmount', () => {
        const removeSpy = vi.spyOn(window, 'removeEventListener');
        const { unmount } = renderHook(() => useMouseIdle());

        unmount();

        const removedEvents = removeSpy.mock.calls.map(([name]) => name);
        expect(removedEvents).toContain('mousemove');
        expect(removedEvents).toContain('mousedown');
        removeSpy.mockRestore();
    });
});
