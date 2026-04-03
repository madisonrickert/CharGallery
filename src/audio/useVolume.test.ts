import { renderHook, act } from '@testing-library/react';
import { createElement } from 'react';
import { useVolume } from './useVolume';
import { AudioContextContext, AudioContextValue } from './useAudioContext';

function createMockAudioContext(): AudioContextValue {
    return {
        audioContext: {} as AudioContextValue['audioContext'],
        setUserVolume: vi.fn(),
    };
}

function renderWithAudioContext(mockCtx: AudioContextValue) {
    const wrapper = ({ children }: { children: React.ReactNode }) =>
        createElement(AudioContextContext.Provider, { value: mockCtx }, children);
    return renderHook(() => useVolume(), { wrapper });
}

describe('useVolume', () => {
    beforeEach(() => { localStorage.clear(); });

    it('defaults to volume enabled', () => {
        const ctx = createMockAudioContext();
        const { result } = renderWithAudioContext(ctx);
        expect(result.current.volumeEnabled).toBe(true);
    });

    it('loads persisted volume state from localStorage', () => {
        localStorage.setItem('sketch-settings:__global', JSON.stringify({ volumeEnabled: false }));
        const ctx = createMockAudioContext();
        const { result } = renderWithAudioContext(ctx);
        expect(result.current.volumeEnabled).toBe(false);
    });

    it('toggleVolume flips state and persists', () => {
        const ctx = createMockAudioContext();
        const { result } = renderWithAudioContext(ctx);

        // Default is true, toggle to false
        act(() => { result.current.toggleVolume(); });
        expect(result.current.volumeEnabled).toBe(false);

        const stored = JSON.parse(localStorage.getItem('sketch-settings:__global')!);
        expect(stored.volumeEnabled).toBe(false);

        act(() => { result.current.toggleVolume(); });
        expect(result.current.volumeEnabled).toBe(true);
    });

    it('syncs volume to AudioContext on mount', () => {
        const ctx = createMockAudioContext();
        renderWithAudioContext(ctx);

        // Default volumeEnabled=true, should set 1
        expect(ctx.setUserVolume).toHaveBeenCalledWith(1);
    });

    it('sets volume to 0 when disabled', () => {
        const ctx = createMockAudioContext();
        const { result } = renderWithAudioContext(ctx);

        act(() => { result.current.toggleVolume(); });
        expect(ctx.setUserVolume).toHaveBeenCalledWith(0);
    });
});
