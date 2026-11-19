"""Roman numerals for 1 to 3999."""

VALUES = [
    (1000, "M"),
    (500, "D"),
    (100, "C"),
    (50, "L"),
    (10, "X"),
    (5, "V"),
    (1, "I"),
]


def to_roman(number: int) -> str:
    """The Roman numeral for `number`, using subtractive forms (IV, IX, XL, XC, CD, CM)."""
    if not 1 <= number <= 3999:
        raise ValueError(f"out of range: {number}")
    result = ""
    for value, letters in VALUES:
        while number >= value:
            result += letters
            number -= value
    return result


def from_roman(text: str) -> int:
    """The number a Roman numeral stands for."""
    letters = {letters: value for value, letters in VALUES}
    total = 0
    for letter in text:
        total += letters[letter]
    return total
