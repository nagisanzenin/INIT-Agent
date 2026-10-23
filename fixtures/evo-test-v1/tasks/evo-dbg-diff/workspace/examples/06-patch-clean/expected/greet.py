import sys


def greet(name, punctuation="!"):
    """Prints a greeting."""
    print("Hello, " + name + punctuation)


def farewell(name):
    print("Goodbye, " + name)


def main(argv):
    names = argv[1:]
    if not names:
        names = ["world"]
    for name in names:
        greet(name)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
