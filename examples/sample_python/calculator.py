"""A simple calculator module demonstrating various Python constructs."""

from typing import Union, Optional
from dataclasses import dataclass


@dataclass
class Result:
    """Represents a calculation result."""
    value: float
    operation: str
    operands: tuple


class Calculator:
    """A basic calculator class with common operations.

    This class demonstrates methods, properties, and decorators.
    """

    def __init__(self, initial_value: float = 0.0):
        """Initialize the calculator with an optional starting value."""
        self._value = initial_value
        self._history: list[Result] = []

    @property
    def value(self) -> float:
        """Get the current value."""
        return self._value

    @property
    def history(self) -> list[Result]:
        """Get the calculation history."""
        return self._history.copy()

    def add(self, x: float) -> float:
        """Add a number to the current value."""
        result = Result(self._value + x, "add", (self._value, x))
        self._value = result.value
        self._history.append(result)
        return self._value

    def subtract(self, x: float) -> float:
        """Subtract a number from the current value."""
        result = Result(self._value - x, "subtract", (self._value, x))
        self._value = result.value
        self._history.append(result)
        return self._value

    def multiply(self, x: float) -> float:
        """Multiply the current value by a number."""
        result = Result(self._value * x, "multiply", (self._value, x))
        self._value = result.value
        self._history.append(result)
        return self._value

    def divide(self, x: float) -> float:
        """Divide the current value by a number.

        Raises:
            ZeroDivisionError: If x is zero.
        """
        if x == 0:
            raise ZeroDivisionError("Cannot divide by zero")
        result = Result(self._value / x, "divide", (self._value, x))
        self._value = result.value
        self._history.append(result)
        return self._value

    def reset(self) -> None:
        """Reset the calculator to zero."""
        self._value = 0.0
        self._history.clear()

    @staticmethod
    def is_valid_number(value: Union[int, float, str]) -> bool:
        """Check if a value can be used as a number."""
        try:
            float(value)
            return True
        except (ValueError, TypeError):
            return False

    @classmethod
    def from_string(cls, value: str) -> "Calculator":
        """Create a calculator from a string value."""
        return cls(float(value))


class ScientificCalculator(Calculator):
    """An extended calculator with scientific operations."""

    def __init__(self, initial_value: float = 0.0, precision: int = 10):
        """Initialize with optional precision setting."""
        super().__init__(initial_value)
        self._precision = precision

    def power(self, exponent: float) -> float:
        """Raise the current value to a power."""
        result = Result(self._value ** exponent, "power", (self._value, exponent))
        self._value = result.value
        self._history.append(result)
        return self._value

    def sqrt(self) -> float:
        """Calculate the square root of the current value."""
        if self._value < 0:
            raise ValueError("Cannot calculate square root of negative number")
        result = Result(self._value ** 0.5, "sqrt", (self._value,))
        self._value = result.value
        self._history.append(result)
        return self._value

    @property
    def precision(self) -> int:
        """Get the precision setting."""
        return self._precision


def create_calculator(scientific: bool = False, initial: float = 0.0) -> Calculator:
    """Factory function to create a calculator instance."""
    if scientific:
        return ScientificCalculator(initial)
    return Calculator(initial)


async def async_calculate(calc: Calculator, operations: list[tuple[str, float]]) -> float:
    """Perform a series of calculations asynchronously."""
    for op, value in operations:
        method = getattr(calc, op, None)
        if method and callable(method):
            method(value)
    return calc.value
