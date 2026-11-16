from pathlib import Path

from PIL import Image, ImageDraw, ImageFont


ROOT = Path(__file__).resolve().parents[1]
ICON_DIR = ROOT / "src-tauri" / "icons"
ICON_DIR.mkdir(parents=True, exist_ok=True)

size = 512
image = Image.new("RGBA", (size, size), "#0c100f")
draw = ImageDraw.Draw(image)
draw.rounded_rectangle((28, 28, size - 28, size - 28), radius=116, fill="#121a16", outline="#29352e", width=8)
draw.rounded_rectangle((72, 72, size - 72, size - 72), radius=94, fill="#a6e77f")

font_path = Path(r"C:\Windows\Fonts\segoeuib.ttf")
font = ImageFont.truetype(str(font_path), 230)
label = "U"
box = draw.textbbox((0, 0), label, font=font)
x = (size - (box[2] - box[0])) / 2
y = (size - (box[3] - box[1])) / 2 - 18
draw.text((x, y), label, font=font, fill="#10200d")

draw.rounded_rectangle((318, 342, 442, 392), radius=25, fill="#10200d")
draw.polygon(((346, 355), (330, 367), (346, 379)), fill="#a6e77f")
draw.line((365, 379, 382, 355), fill="#a6e77f", width=7)
draw.polygon(((401, 355), (417, 367), (401, 379)), fill="#a6e77f")

image.save(ICON_DIR / "icon.png", optimize=True)
image.save(ICON_DIR / "icon.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print(ICON_DIR / "icon.ico")
