# Core matrix

`core.matrix/1` implements explicit channel mapping from
[MaaC/1 §18.4](../MaaC-1-Specification.md#184-corematrix1).

```maac
node duplicate {
  type = "core.matrix/1";
  config = {
    inputs = 1;
    outputs = 2;
    coefficients = [[1], [1/2]];
  };
}
```

`inputs` and `outputs` are required positive integers. This engine supports one
or two channels on each side; larger positive dimensions fail with
`E_CAPABILITY`. The single audio `in` requires exactly one connection with
`inputs` channels; audio `out` has `outputs` channels.

`coefficients` is a required rectangular array with one row per output channel
and one column per input channel. Entries are finite dimensionless rational
values. Signed values, zero, and values greater than one are allowed. There are
no parameters or implicit defaults for the matrix.

For each output row, processing starts with zero and adds each
`coefficient * input` product in ascending input-channel order. Coefficients are
converted to binary64 once during preparation. Products and running sums must
remain finite or rendering fails with `E_NONFINITE`. There is no normalization,
clipping, smoothing, or fused multiply-add substitution. The matrix has no
history and zero technical latency.

The processor uses the existing plan versions with the `matrix` processor tag,
explicit `inputs` and `outputs`, and rational coefficient rows. Source-free
retained-plan replay preserves the mapping. Production execution identity
includes the coefficient values and dimensions.

Execution accounting charges `2 * inputs * outputs` normalized units per
rendered frame, including silent nodes, nodes disconnected from the project
output, and the entire render tail. Processing uses fixed-size prepared
coefficient and sample buffers within the mono/stereo capability.

The [complete example](../examples/core-matrix.maac) demonstrates explicit
channel mapping. Acceptance covers orientation, signed and fractional weights,
duplication and downmixing, strict shape and unit validation, overflow failures,
resource limits, and repeatable retained-plan CLI rendering.
