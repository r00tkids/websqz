let LnMixerPred = (byteMask) => {
    return {
        pred: () => {
            return probStretch(NOrderByteHashMap.get(ctx ^ bitCtx).prob / U24Max);
        },
        learn: (bit) => {
        },
    };
};