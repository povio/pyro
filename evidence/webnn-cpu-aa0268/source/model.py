"""Serialize one small MLModel from Apple's published Core ML protobuf schema.

The source model is a two-node, two-input-feature affine layer followed by ReLU.
There are no Python model-format packages or generated project dependencies.
"""
import struct
import sys
from pathlib import Path

def varint(value):
    if value < 0:
        raise ValueError("expected a nonnegative protobuf integer")
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)

def integer(field, value):
    return varint(field << 3) + varint(value)

def field(field_number, value):
    return varint((field_number << 3) | 2) + varint(len(value)) + value

def feature(name, size):
    array_type = integer(1, size) + integer(2, 65568)  # FLOAT32
    return field(1, name.encode()) + field(3, field(5, array_type))

def weights(vals):
    return field(1, struct.pack('<'+'f'*len(vals), *vals))

def layer(name, input_name, output_name, operation):
    return field(1, name.encode()) + field(2, input_name.encode()) + field(3, output_name.encode()) + operation

def affine_relu(w, b):
    outputs = len(w)
    if not outputs or outputs != len(b) or not w[0]:
        raise ValueError('bias and nonempty output dimension must match')
    inputs = len(w[0])
    if any(len(row) != inputs for row in w):
        raise ValueError('matrix rows must have equal length')
    description = field(1, feature('x', inputs)) + field(10, feature('y', outputs))
    dense = integer(1, inputs) + integer(2, outputs) + integer(10, 1)
    dense += field(20, weights([e for row in w for e in row])) + field(21, weights(b))
    layers = field(1, layer('affine', 'x', 'z', field(140, dense)))
    layers += field(1, layer('relu', 'z', 'y', field(130, field(10, b''))))
    return integer(1, 4) + field(2, description) + field(500, layers)

if __name__ == '__main__':
    Path(sys.argv[1]).write_bytes(affine_relu([[1,2],[-1,1],[0.5,-2]],[-1,0.5,2]))
