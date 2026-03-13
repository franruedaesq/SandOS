import urllib.request
import sys

url = "https://raw.githubusercontent.com/aselectroworks/Arduino-FT6336U/master/src/FT6336U.cpp"
try:
    with urllib.request.urlopen(url) as response:
        content = response.read().decode('utf-8')
        print(content)
except Exception as e:
    print(f"Error: {e}", file=sys.stderr)

url2 = "https://raw.githubusercontent.com/aselectroworks/Arduino-FT6336U/master/src/FT6336U.h"
try:
    with urllib.request.urlopen(url2) as response:
        content = response.read().decode('utf-8')
        print(content)
except Exception as e:
    print(f"Error: {e}", file=sys.stderr)
