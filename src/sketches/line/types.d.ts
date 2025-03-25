export interface AudioGroup {
    analyser: AnalyserNode;
    chordGain: GainNode;
    sourceGain: GainNode;
    sourceLfo: OscillatorNode;
    lfoGain: GainNode;
    filter: BiquadFilterNode;
    filter2: BiquadFilterNode;
    filterGain: GainNode;
    setFrequency: (freq: number) => void;
    setNoiseFrequency: (freq: number) => void;
    setVolume: (volume: number) => void;
    setBackgroundVolume: (volume: number) => void;
}