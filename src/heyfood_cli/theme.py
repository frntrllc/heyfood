from rich.theme import Theme


HEYFOOD_COLORS = {
    "accent": "#9bc53d",
    "bright": "#edeae0",
    "muted": "#74808d",
    "info": "#76b8e9",
    "warning": "#efc15d",
    "danger": "#f17c75",
}


HEYFOOD_THEME = Theme(
    {
        "green": HEYFOOD_COLORS["accent"],
        "yellow": HEYFOOD_COLORS["warning"],
        "red": HEYFOOD_COLORS["danger"],
        "blue": HEYFOOD_COLORS["info"],
        "hey.accent": HEYFOOD_COLORS["accent"],
        "hey.bright": HEYFOOD_COLORS["bright"],
        "hey.muted": HEYFOOD_COLORS["muted"],
        "hey.info": HEYFOOD_COLORS["info"],
        "hey.warning": HEYFOOD_COLORS["warning"],
        "hey.danger": HEYFOOD_COLORS["danger"],
    }
)
