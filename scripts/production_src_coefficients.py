#!/usr/bin/env python3
"""Certified maac.src.kaiser/1 tables; Python standard library only.

Every real value is enclosed by integer endpoints scaled by 2**256. Arithmetic
rounds outward. pi = 16 atan(1/5)-4 atan(1/239); 256 alternating terms enclose
both arctangents with the next term as error. Sine is reduced using the exact
rational argument to [-pi/2,pi/2]; 80 Taylor terms plus the next-term bound
suffice. I0 uses 128 nonnegative terms and bounds its remainder by twice the
next term (all subsequent ratios <=49/129**2 <1/2). No unverified libm function
participates. Per-phase real normalization precedes the single float rounding.
Both interval endpoints must round to identical IEEE binary64 bits. Failure is
fatal, including any coefficient too near a rounding boundary for this proof.

Prototype common factors 1/(pi*I0(14)) cancel during phase normalization, so we
bound sin(pi*24*q/(25*m))*I0(14*sqrt(1-(q/K)**2))/q, with its analytic
q=0 limit. The I0 helper takes the squared argument divided by four.
Symmetry permits storing q=0..K only. --verify recomputes the certificate and
compares all bytes; the default writes tables and certificate.
"""
import argparse
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import struct
import sys

BITS = 256
S = 1 << BITS
ROOT = Path(__file__).resolve().parents[1] / 'src' / 'production_src'


def divceil(a, b):
    return -((-a) // b)


class I:
    __slots__ = ('lo', 'hi')

    def __init__(self, lo, hi=None):
        self.lo, self.hi = lo, lo if hi is None else hi

    @classmethod
    def rational(cls, a, b=1):
        if b <= 0:
            raise ValueError("interval denominator must be positive")
        return cls(a*S//b, divceil(a*S, b))

    def __add__(self, other):
        return I(self.lo+other.lo, self.hi+other.hi)

    def __neg__(self):
        return I(-self.hi, -self.lo)

    def __sub__(self, other):
        return self + -other

    def __mul__(self, other):
        terms = (self.lo*other.lo, self.lo*other.hi,
                 self.hi*other.lo, self.hi*other.hi)
        return I(min(terms)//S, divceil(max(terms), S))

    def divide(self, other):
        if other.lo <= 0:
            raise ValueError("division requires a strictly positive denominator interval")
        terms = [(a*S, b) for a in (self.lo,self.hi)
                 for b in (other.lo,other.hi)]
        return I(min(a//b for a,b in terms), max(divceil(a,b) for a,b in terms))

    def scale(self, n, d=1):
        if n < 0:
            return -self.scale(-n, d)
        if d <= 0:
            raise ValueError("scale denominator must be positive")
        return I(self.lo*n//d, divceil(self.hi*n,d))


def arctan_inverse(q):
    result = I(0)
    power = q
    for k in range(256):
        term = I.rational(1, (2*k+1)*power)
        result = result + (term if k % 2 == 0 else -term)
        power *= q*q
    # 256 terms end with a negative term; remainder is positive and <= next.
    return I(result.lo, result.hi + divceil(S, 513*power))


PI = arctan_inverse(5).scale(16) - arctan_inverse(239).scale(4)


def sine_rational_pi(n, d):
    # Map rational n/d modulo two to [-1/2,1/2] using exact integers.
    n %= 2*d
    sign = 1
    if n > d:
        n -= d
        sign = -1
    if 2*n > d:
        n = d-n
    if n == 0:
        return I(0)
    x = PI.scale(n,d)
    x2 = x*x
    term, result = x, x
    for k in range(1,80):
        term = (term*x2).scale(1,(2*k)*(2*k+1))
        result = result + (term if k % 2 == 0 else -term)
    following = (term*x2).scale(1,160*161)
    return I(result.lo-following.hi, result.hi+following.hi).scale(sign)


def i0_squared_argument(t):
    term = result = I(S)
    for k in range(1,128):
        term = (term*t).scale(1,k*k)
        result = result+term
    following = (term*t).scale(1,128*128)
    # All subsequent ratios at most 49/129²; geometric tail < 2*following.
    return I(result.lo,result.hi+2*following.hi)


def certified_round(value):
    lo = float(Fraction(value.lo,S))
    hi = float(Fraction(value.hi,S))
    rounded_lo = struct.pack('<d',lo)
    if rounded_lo != struct.pack('<d',hi):
        raise ArithmeticError('rounding not established')
    return rounded_lo


def table(up, down):
    m=max(up,down)
    k=256*m
    prototype=[]
    for q in range(k+1):
        window=i0_squared_argument(I.rational(49*(k*k-q*q),k*k))
        if q:
            p=(sine_rational_pi(24*q,25*m)*window).scale(1,q)
        else:
            p=PI.scale(24,25*m)*window
        prototype.append(p)
    totals=[I(0) for _ in range(up)]
    for q in range(-k,k+1):
        totals[q % up]=totals[q % up]+prototype[abs(q)]
    out=bytearray()
    max_width=0
    for q,p in enumerate(prototype):
        coefficient=p.divide(totals[q % up])
        out.extend(certified_round(coefficient))
        max_width=max(max_width, coefficient.hi-coefficient.lo)
    return bytes(out),max_width


def verify_equal(actual, expected, label):
    # Verification and proof gates must remain active under python -O and
    # PYTHONOPTIMIZE; assertions are deliberately not used for either purpose.
    if actual != expected:
        raise ValueError(f'{label}: generated result does not match checked-in artifact')


def self_test():
    # Run this with both python and python -O. Each invalid input must raise its
    # specific error; a silent acceptance or unrelated exception fails the test.
    cases = [
        ('ambiguous rounding', lambda: certified_round(I(0, S)), ArithmeticError),
        ('zero rational denominator', lambda: I.rational(1, 0), ValueError),
        ('denominator containing zero', lambda: I(S).divide(I(-1, 1)), ValueError),
        ('zero scale denominator', lambda: I(S).scale(1, 0), ValueError),
        ('mismatched artifact', lambda: verify_equal(b'a', b'b', 'probe'), ValueError),
    ]
    for label, operation, expected_error in cases:
        try:
            operation()
        except expected_error:
            continue
        raise RuntimeError(f'{label}: invalid input was accepted')
    verify_equal(certified_round(I(S)), struct.pack('<d', 1.0), 'exact rounding')
    print(f'certificate rejection self-test passed (optimization={sys.flags.optimize})')


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    mode=parser.add_mutually_exclusive_group()
    mode.add_argument('--verify',action='store_true')
    mode.add_argument('--self-test',action='store_true', help='check failure paths; also run with python -O')
    args=parser.parse_args()
    if args.self_test:
        self_test()
        return
    report={'identity':'maac.src.kaiser/1', 'generator':'production_src_coefficients.py/1',
            'generator_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            'fractional_interval_bits':BITS, 'coefficient_rounding':'both outward interval endpoints round to identical binary64',
            'storage':'little-endian binary64 h(q) for q=0..K; h(-q)=h(q)', 'tables':{}}
    for rate,up,down in [(44100,147,160),(96000,2,1)]:
        data,width=table(up,down)
        name=f'{rate}.f64le'
        if args.verify:
            verify_equal((ROOT/name).read_bytes(), data, name)
        else:
            (ROOT/name).write_bytes(data)
        report['tables'][str(rate)]={'file':name,'up':up,'down':down,'half_length':256*max(up,down),
                                    'sha256':hashlib.sha256(data).hexdigest(), 'bytes':len(data),
                                    'maximum_interval_width_in_2_to_minus_256_units':width}
    payload=json.dumps(report,indent=2)+'\n'
    if args.verify:
        verify_equal((ROOT/'certificate.json').read_text(), payload, 'certificate.json')
    else:
        (ROOT/'certificate.json').write_text(payload)
    print(payload,end='')


if __name__=='__main__':
    main()
