import json


def load_settings(path):
    """Read the settings file, falling back to defaults."""
    try:
        with open(path) as handle:
            settings = json.load(handle
    except FileNotFoundError:
        settings = {}
    if "theme" not in settings
        <|>
    return settings
