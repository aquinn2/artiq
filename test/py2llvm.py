import unittest
import ast
import inspect
from fractions import Fraction

from llvm import ee as le

from artiq.language.core import int64
from artiq.py2llvm.infer_types import infer_function_types
from artiq.py2llvm import base_types
from artiq.py2llvm.module import Module


def test_types(choice):
    a = 2          # promoted later to int64
    b = a + 1      # initially int32, becomes int64 after a is promoted
    c = b//2       # initially int32, becomes int64 after b is promoted
    d = 4          # stays int32
    x = int64(7)
    a += x         # promotes a to int64
    foo = True
    bar = None

    if choice and foo and not bar:
        return d
    else:
        return x + c


class FunctionTypesCase(unittest.TestCase):
    def setUp(self):
        self.ns = infer_function_types(
            None, ast.parse(inspect.getsource(test_types)),
            dict())

    def test_base_types(self):
        self.assertIsInstance(self.ns["foo"], base_types.VBool)
        self.assertIsInstance(self.ns["bar"], base_types.VNone)
        self.assertIsInstance(self.ns["d"], base_types.VInt)
        self.assertEqual(self.ns["d"].nbits, 32)
        self.assertIsInstance(self.ns["x"], base_types.VInt)
        self.assertEqual(self.ns["x"].nbits, 64)

    def test_promotion(self):
        for v in "abc":
            self.assertIsInstance(self.ns[v], base_types.VInt)
            self.assertEqual(self.ns[v].nbits, 64)

    def test_return(self):
        self.assertIsInstance(self.ns["return"], base_types.VInt)
        self.assertEqual(self.ns["return"].nbits, 64)


class CompiledFunction:
    def __init__(self, function, param_types):
        module = Module()
        funcdef = ast.parse(inspect.getsource(function)).body[0]
        self.function, self.retval = module.compile_function(
            funcdef, param_types)
        self.argval = [param_types[arg.arg] for arg in funcdef.args.args]
        self.ee = module.get_ee()

    def __call__(self, *args):
        args_llvm = [
            le.GenericValue.int(av.get_llvm_type(), a)
            for av, a in zip(self.argval, args)]
        result = self.ee.run_function(self.function, args_llvm)
        if isinstance(self.retval, base_types.VBool):
            return bool(result.as_int())
        elif isinstance(self.retval, base_types.VInt):
            return result.as_int_signed()
        else:
            raise NotImplementedError


def is_prime(x):
    d = 2
    while d*d <= x:
        if not x % d:
            return False
        d += 1
    return True


def simplify_encode(a, b):
    f = Fraction(a, b)
    return f.numerator*1000 + f.denominator


def arith_encode(op, a, b, c, d):
    if op == 1:
        f = Fraction(a, b) - Fraction(c, d)
    elif op == 2:
        f = Fraction(a, b) + Fraction(c, d)
    elif op == 3:
        f = Fraction(a, b) * Fraction(c, d)
    else:
        f = Fraction(a, b) / Fraction(c, d)
    return f.numerator*1000 + f.denominator


is_prime_c = CompiledFunction(is_prime, {"x": base_types.VInt()})
simplify_encode_c = CompiledFunction(
    simplify_encode, {"a": base_types.VInt(), "b": base_types.VInt()})
arith_encode_c = CompiledFunction(
    arith_encode, {
        "op": base_types.VInt(),
        "a": base_types.VInt(), "b": base_types.VInt(),
        "c": base_types.VInt(), "d": base_types.VInt()})


class CodeGenCase(unittest.TestCase):
    def test_is_prime(self):
        for i in range(200):
            self.assertEqual(is_prime_c(i), is_prime(i))

    def test_frac_simplify(self):
        for a in range(5, 20):
            for b in range(5, 20):
                self.assertEqual(
                    simplify_encode_c(a, b), simplify_encode(a, b))

    def _test_frac_arith(self, op):
        for a in range(5, 10):
            for b in range(5, 10):
                for c in range(5, 10):
                    for d in range(5, 10):
                        self.assertEqual(
                            arith_encode_c(op, a, b, c, d),
                            arith_encode(op, a, b, c, d))

    def test_frac_add(self):
        self._test_frac_arith(0)

    def test_frac_sub(self):
        self._test_frac_arith(1)

    def test_frac_mul(self):
        self._test_frac_arith(2)

    def test_frac_div(self):
        self._test_frac_arith(3)
