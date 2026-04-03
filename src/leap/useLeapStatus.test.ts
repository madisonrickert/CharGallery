import { renderHook, act } from '@testing-library/react';
import { useLeapStatus } from './useLeapStatus';
import { LeapProcessStatus } from './leapStatus';
import { ElectronAPI } from '@/types/electron-api';

function mockElectronAPI(overrides: Partial<ElectronAPI> = {}): ElectronAPI {
    return {
        getLeapProcessStatus: vi.fn().mockResolvedValue('running'),
        onLeapProcessStatus: vi.fn().mockReturnValue(vi.fn()),
        startLeapProcess: vi.fn(),
        stopLeapProcess: vi.fn(),
        startPowerSaveBlocker: vi.fn().mockResolvedValue(undefined),
        stopPowerSaveBlocker: vi.fn().mockResolvedValue(undefined),
        ...overrides,
    };
}

describe('useLeapStatus', () => {
    afterEach(() => {
        delete (window as unknown as Record<string, unknown>).electronAPI;
    });

    it('returns default statuses when electronAPI is not available', () => {
        const { result } = renderHook(() => useLeapStatus());
        expect(result.current.processStatus).toBe('not-started');
        expect(result.current.connectionStatus).toBe('disconnected');
    });

    it('queries initial process status from electronAPI', async () => {
        window.electronAPI = mockElectronAPI();

        const { result } = renderHook(() => useLeapStatus());

        // Wait for the async getLeapProcessStatus to resolve
        await act(async () => {});

        expect(window.electronAPI!.getLeapProcessStatus).toHaveBeenCalled();
        expect(result.current.processStatus).toBe('running');
    });

    it('subscribes to process status updates', async () => {
        let statusCallback: ((status: LeapProcessStatus) => void) | null = null;
        const cleanup = vi.fn();

        window.electronAPI = mockElectronAPI({
            onLeapProcessStatus: vi.fn((cb: (status: LeapProcessStatus) => void) => {
                statusCallback = cb;
                return cleanup;
            }),
        });

        const { result, unmount } = renderHook(() => useLeapStatus());
        await act(async () => {});

        expect(statusCallback).not.toBeNull();

        // Simulate a status update from main process
        act(() => {
            statusCallback!('exited');
        });
        expect(result.current.processStatus).toBe('exited');

        // Cleanup on unmount
        unmount();
        expect(cleanup).toHaveBeenCalled();
    });

    it('setConnectionStatus updates connection status', () => {
        const { result } = renderHook(() => useLeapStatus());

        act(() => {
            result.current.setConnectionStatus('streaming');
        });
        expect(result.current.connectionStatus).toBe('streaming');
    });

    it('derives process status as "external" when connected without electronAPI', () => {
        const { result } = renderHook(() => useLeapStatus());

        act(() => {
            result.current.setConnectionStatus('connected');
        });
        expect(result.current.processStatus).toBe('external');
    });

    it('derives process status as "not-started" when disconnected without electronAPI', () => {
        const { result } = renderHook(() => useLeapStatus());
        expect(result.current.processStatus).toBe('not-started');
    });

    it('startProcess calls electronAPI and updates status', async () => {
        window.electronAPI = mockElectronAPI({
            getLeapProcessStatus: vi.fn().mockResolvedValue('exited'),
            startLeapProcess: vi.fn().mockResolvedValue('running'),
        });

        const { result } = renderHook(() => useLeapStatus());
        await act(async () => {});

        await act(async () => {
            await result.current.startProcess();
        });

        expect(window.electronAPI!.startLeapProcess).toHaveBeenCalled();
        expect(result.current.processStatus).toBe('running');
    });

    it('stopProcess calls electronAPI and updates status', async () => {
        window.electronAPI = mockElectronAPI({
            getLeapProcessStatus: vi.fn().mockResolvedValue('running'),
            stopLeapProcess: vi.fn().mockResolvedValue('exited'),
        });

        const { result } = renderHook(() => useLeapStatus());
        await act(async () => {});

        await act(async () => {
            await result.current.stopProcess();
        });

        expect(window.electronAPI!.stopLeapProcess).toHaveBeenCalled();
        expect(result.current.processStatus).toBe('exited');
    });

    it('startProcess is a no-op without electronAPI', async () => {
        const { result } = renderHook(() => useLeapStatus());

        // Should not throw
        await act(async () => {
            await result.current.startProcess();
        });
        expect(result.current.processStatus).toBe('not-started');
    });
});
