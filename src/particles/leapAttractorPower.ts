/**
 * Configuration for the Leap grab-strength-to-attractor-power mapping.
 * Each sketch can tune these constants independently.
 */
export interface LeapAttractorPowerConfig {
    /** Smoothing factor for power increase (0–1). Lower = slower attack. */
    attackSpeed: number;
    /** Multiplier for power decrease each frame when not grabbing (0–1). Lower = faster decay. */
    decaySpeed: number;
    /** Grab strength at or below this value counts as "not grabbing". */
    grabThreshold: number;
    /** If set, power is zeroed when it decays below this floor. */
    powerFloor?: number;
}

/**
 * Computes the next attractor power based on Leap hand grab state.
 *
 * When the hand is grabbing (above threshold), power ramps up toward a target
 * derived from grab strength and palm depth. When released, power decays
 * geometrically. The formula maps grab strength through a 1.5 power curve and
 * modulates by palm Z depth so closer hands exert more force.
 *
 * This is a pure function — it returns the new power value without mutating anything.
 */
export function computeLeapAttractorPower(
    currentPower: number,
    grabStrength: number,
    palmZ: number,
    config: LeapAttractorPowerConfig,
): number {
    if (grabStrength <= config.grabThreshold) {
        const decayed = currentPower * config.decaySpeed;
        if (config.powerFloor != null && decayed < config.powerFloor) {
            return 0;
        }
        return decayed;
    }

    const grabComponent = Math.pow(grabStrength, 1.5);
    const depthModulator = Math.pow(5, (-palmZ + 350) / 160);
    const wantedPower = grabComponent * depthModulator;
    return currentPower * (1 - config.attackSpeed) + wantedPower * config.attackSpeed;
}
