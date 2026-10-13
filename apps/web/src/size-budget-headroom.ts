// THE HEADROOM POLICY, HELD AGAINST THE RECORDING (owner, 2026-10-13).
//
// `size-budget.json`'s `headroom` block states the margin each kind of line keeps above its
// recorded measurement: 25% per artifact, 25% for the page lines, 10% for a total. Until this
// existed nothing checked it -- the test that used the numbers asserted a property of the budget
// VALUES -- so the lines drifted under their policy one re-record at a time, and when the stopgap
// for split's optional content grew the base module 14.9% its line was left at 6%. This asks the
// question directly: is every budget at least its recorded measurement times one plus its policy?

/** One budget line, with the policy that applies to it. */
export interface HeadroomLine {
  /** What the line budgets, as the failure names it: `engines/qpdf.wasm`, `render total`. */
  name: string;
  /** `measured_brotli`: the recording the budget was set from. */
  measured: number;
  /** `budget_brotli`. */
  budget: number;
  /** The headroom the policy requires, as a fraction: 0.25, 0.10. */
  policy: number;
}

/**
 * The smallest budget the policy allows over `measured`: measured x (1 + policy), rounded UP, in
 * whole percentage points so that no float rounding lets a budget one byte short pass.
 */
export function policyMinimum(measured: number, policy: number): number {
  const points = Math.round(policy * 100);
  // WHOLE PERCENTAGE POINTS ONLY: a policy of 0.125 would round to 13% here without saying so.
  if (Math.abs(policy * 100 - points) > 1e-9) {
    throw new Error(`a headroom policy of ${policy} is not a whole percentage`);
  }
  return Math.ceil((measured * (100 + points)) / 100);
}

/** Every line whose budget is under its policy minimum, each as a sentence naming the numbers. */
export function headroomFindings(lines: HeadroomLine[]): string[] {
  return lines
    .filter((line) => line.budget < policyMinimum(line.measured, line.policy))
    .map((line) => {
      const minimum = policyMinimum(line.measured, line.policy);
      const has = ((line.budget / line.measured - 1) * 100).toFixed(1);
      return (
        `${line.name}: budget ${line.budget} is under ${minimum}, its recording ` +
        `${line.measured} plus the ${Math.round(line.policy * 100)}% policy -- ${has}% headroom`
      );
    });
}
