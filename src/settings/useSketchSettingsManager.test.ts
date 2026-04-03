import { renderHook, act } from '@testing-library/react';
import { useSketchSettingsManager } from './useSketchSettingsManager';
import { SketchConstructor } from '@/sketch/Sketch';

const makeSketchClass = (id: string, settings = {}): SketchConstructor => {
    return { id, settings } as unknown as SketchConstructor;
};

describe('useSketchSettingsManager', () => {
    beforeEach(() => {
        localStorage.clear();
        vi.useFakeTimers();
    });
    afterEach(() => { vi.useRealTimers(); });

    it('loads settings with defaults', () => {
        const sketchClass = makeSketchClass('test', {
            speed: { default: 5, category: 'dev', label: 'Speed' },
        });
        const { result } = renderHook(() => useSketchSettingsManager(sketchClass));

        expect(result.current.sketchId).toBe('test');
        expect(result.current.settings).toEqual({ speed: 5 });
    });

    it('loads persisted settings from localStorage', () => {
        localStorage.setItem('sketch-settings:test', JSON.stringify({ speed: 10 }));
        const sketchClass = makeSketchClass('test', {
            speed: { default: 5, category: 'dev', label: 'Speed' },
        });
        const { result } = renderHook(() => useSketchSettingsManager(sketchClass));

        expect(result.current.settings).toEqual({ speed: 10 });
    });

    it('setSetting updates state and persists to localStorage', () => {
        const sketchClass = makeSketchClass('test', {
            speed: { default: 5, category: 'dev', label: 'Speed' },
        });
        const { result } = renderHook(() => useSketchSettingsManager(sketchClass));

        act(() => { result.current.setSetting('speed', 20); });

        expect(result.current.settings).toEqual({ speed: 20 });
        const stored = JSON.parse(localStorage.getItem('sketch-settings:test')!);
        expect(stored.speed).toBe(20);
    });

    it('computes restartKey from requiresRestart settings', () => {
        const sketchClass = makeSketchClass('test', {
            speed: { default: 5, category: 'dev', label: 'Speed', requiresRestart: true },
            color: { default: 'red', category: 'dev', label: 'Color' },
        });
        const { result } = renderHook(() => useSketchSettingsManager(sketchClass));

        // restartKey should only include the requiresRestart setting
        expect(result.current.restartKey).toBe('speed=5');
    });

    it('debounces restartKey updates', () => {
        const sketchClass = makeSketchClass('test', {
            speed: { default: 5, category: 'dev', label: 'Speed', requiresRestart: true },
        });
        const { result } = renderHook(() => useSketchSettingsManager(sketchClass));
        const initialKey = result.current.restartKey;

        act(() => { result.current.setSetting('speed', 10); });

        // restartKey should NOT change immediately (debounced)
        expect(result.current.restartKey).toBe(initialKey);

        // After the debounce timeout, it should update
        act(() => { vi.advanceTimersByTime(300); });
        expect(result.current.restartKey).toBe('speed=10');
    });

    it('falls back to class name when id is missing', () => {
        const sketchClass = { name: 'FallbackSketch', settings: {} } as unknown as SketchConstructor;
        const { result } = renderHook(() => useSketchSettingsManager(sketchClass));
        expect(result.current.sketchId).toBe('FallbackSketch');
    });
});
