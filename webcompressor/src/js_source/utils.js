let probStretch = (prob) => {
    return Math.log(prob / (1. - prob));
};
let probSquash = (prob) => {
    return 1. / (1. + Math.exp(-prob));
};