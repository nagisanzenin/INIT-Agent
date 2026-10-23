import sys


def greet(name):
    print("Hello, " + name)


def farewell(name):
    print("Goodbye, " + name)


def main(argv):
    names = argv[1:]
    for name in names:
        greet(name)
    for name in names:
        farewell(name)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
