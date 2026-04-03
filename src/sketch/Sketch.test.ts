import * as THREE from 'three';
import { Sketch, SketchAudioContext } from './Sketch';

/** Concrete test subclass that records calls to the step hook. */
class TestSketch extends Sketch {
    initCalled = false;
    stepCalls: number[] = [];

    init() { this.initCalled = true; }

    protected step(currentTimeMs: number) {
        this.stepCalls.push(currentTimeMs);
    }

    // Expose protected members for testing
    get _isIdle() { return this.isIdle; }
    set _isIdle(v: boolean) { this.isIdle = v; }
    get _idleTimeoutSeconds() { return this.idleTimeoutSeconds; }
    set _idleTimeoutSeconds(v: number) { this.idleTimeoutSeconds = v; }
    doMarkInteraction(ts?: number) { this.markInteraction(ts); }
}

function createTestSketch(): TestSketch {
    const canvas = document.createElement('canvas');
    const renderer = { domElement: canvas } as unknown as THREE.WebGLRenderer;
    const audioContext = {} as SketchAudioContext;
    return new TestSketch(renderer, audioContext);
}

describe('Sketch.animate', () => {
    it('calls step() when not idle', () => {
        const sketch = createTestSketch();
        sketch.animate(16);
        expect(sketch.stepCalls).toHaveLength(1);
    });

    it('skips step() when idle', () => {
        const sketch = createTestSketch();
        sketch._isIdle = true;
        sketch.animate(16);
        expect(sketch.stepCalls).toHaveLength(0);
    });

    it('marks interaction when leap hands are active', () => {
        const sketch = createTestSketch();
        sketch['leapHands'] = { activeHandCount: 2 } as any;

        // Set a long idle timeout so updateIdleState won't immediately re-idle
        sketch._idleTimeoutSeconds = 9999;

        sketch.animate(16);

        // markInteraction should have been called, so step should run
        expect(sketch.stepCalls).toHaveLength(1);
    });

    it('does not throw when leapHands is undefined', () => {
        const sketch = createTestSketch();
        sketch.animate(16);
        expect(sketch.stepCalls).toHaveLength(1);
    });
});

describe('Sketch.destroy', () => {
    it('disposes leapHands if present', () => {
        const sketch = createTestSketch();
        const dispose = vi.fn();
        sketch['leapHands'] = { dispose } as any;

        sketch.destroy();
        expect(dispose).toHaveBeenCalledTimes(1);
    });

    it('does not throw when leapHands is undefined', () => {
        const sketch = createTestSketch();
        expect(() => sketch.destroy()).not.toThrow();
    });
});

describe('Sketch idle/screensaver tracking', () => {
    it('transitions to idle after timeout', () => {
        const sketch = createTestSketch();
        sketch._idleTimeoutSeconds = 1;

        sketch.doMarkInteraction(1000);
        sketch['updateIdleState'](1000);
        expect(sketch._isIdle).toBe(false);

        sketch['updateIdleState'](3000); // 2 seconds after last interaction
        expect(sketch._isIdle).toBe(true);
    });

    it('fires screensaver callback', () => {
        const sketch = createTestSketch();
        sketch['screenSaverTimeoutSeconds'] = 1;
        const callback = vi.fn();
        sketch.updateScreenSaverCallback = callback;

        sketch.doMarkInteraction(0);
        sketch['updateIdleState'](500);
        expect(callback).toHaveBeenCalledWith(false);

        sketch['updateIdleState'](2000);
        expect(callback).toHaveBeenCalledWith(true);
    });
});
