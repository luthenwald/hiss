import collections
import sys


def count_words(path):
    """Return the ten most common words in the file at path."""
    counter = collections.Counter()
    with open(path) as handle:
        for line in handle:
            for word in line.spl<|>
    return counter.most_common(10)


if __name__ == "__main__":
    print(count_words(sys.argv[1]))
