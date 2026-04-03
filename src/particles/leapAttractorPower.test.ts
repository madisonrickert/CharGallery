import { computeLeapAttractorPower, LeapAttractorPowerConfig } from './leapAttractorPower';

const LINE_CONFIG: LeapAttractorPowerConfig = {
    attackSpeed: 0.005,
    decaySpeed: 0.5,
    grabThreshold: 0, // Line uses exact zero
};

const DOTS_CONFIG: LeapAttractorPowerConfig = {
    attackSpeed: 0.005,
    decaySpeed: 0.5,
    grabThreshold: 0.1,
    powerFloor: 0.05,
};

describe('computeLeapAttractorPower', () => {
    it('decays power when grab strength is below threshold', () => {
        const result = computeLeapAttractorPower(10, 0, 350, LINE_CONFIG);
        expect(result).toBe(10 * 0.5);
    });

    it('increases power when grabbing', () => {
        const result = computeLeapAttractorPower(0, 1.0, 350, LINE_CONFIG);
        // grabComponent = 1^1.5 = 1, depthModulator = 5^0 = 1, wantedPower = 1
        // result = 0 * 0.995 + 1 * 0.005 = 0.005
        expect(result).toBeCloseTo(0.005);
    });

    it('interpolates between current and wanted power', () => {
        const result = computeLeapAttractorPower(100, 1.0, 350, LINE_CONFIG);
        // wantedPower = 1, result = 100 * 0.995 + 1 * 0.005 = 99.505
        expect(result).toBeCloseTo(99.505);
    });

    it('increases power with closer palm (lower Z)', () => {
        const farResult = computeLeapAttractorPower(0, 1.0, 500, LINE_CONFIG);
        const closeResult = computeLeapAttractorPower(0, 1.0, 200, LINE_CONFIG);
        expect(closeResult).toBeGreaterThan(farResult);
    });

    it('zeroes power below powerFloor when configured', () => {
        const result = computeLeapAttractorPower(0.04, 0, 350, DOTS_CONFIG);
        // 0.04 * 0.5 = 0.02, which is below powerFloor 0.05
        expect(result).toBe(0);
    });

    it('does not zero power above powerFloor', () => {
        const result = computeLeapAttractorPower(1.0, 0, 350, DOTS_CONFIG);
        // 1.0 * 0.5 = 0.5, above powerFloor
        expect(result).toBe(0.5);
    });

    it('uses Dots grab threshold (0.1)', () => {
        // grabStrength 0.05 is below Dots threshold but above Line threshold
        const dotsResult = computeLeapAttractorPower(10, 0.05, 350, DOTS_CONFIG);
        const lineResult = computeLeapAttractorPower(10, 0.05, 350, LINE_CONFIG);

        // Dots treats this as decay, Line treats it as a grab
        expect(dotsResult).toBe(10 * 0.5); // decay
        expect(lineResult).toBeGreaterThan(0); // attack (not simple decay)
        expect(lineResult).not.toBe(10 * 0.5);
    });
});
