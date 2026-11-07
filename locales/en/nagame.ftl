# Nagame English messages. Use stable semantic keys, not UI text as identifiers.
# Keep complete sentences and named variables here, including plural variants.

menu-file = File

menu-list = Browse

menu-view = View

menu-other = Tools

open-image-menu = Open image…

open-folder-menu = Open folder…

save-as-menu = Save as…

settings-menu = Settings…

open-image = Open image

open-folder = Open folder

reload = Reload

save-as = Save as

copy-image = Copy image

copy-file-path = Copy file path

copy-path = Copy path

reveal-folder = Open containing folder

reveal-image-folder = Open image folder

previous-image = Previous image

next-image = Next image

skip-previous = Skip backward

skip-next = Skip forward

first-image = First image

last-image = Last image

previous-folder = Previous folder

next-folder = Next folder

previous-sibling = Previous sibling folder

next-sibling = Next sibling folder

parent-folder = Parent folder

sort-name = Sort by name

sort-modified = Sort by modified date

sort-size = Sort by size

sort-extension = Sort by extension

random = Random

reverse-order = Reverse order

image-info = Image information

zoom-in = Zoom in

zoom-out = Zoom out

fit-window = Fit to window

actual-size = Actual size

rotate-right = Rotate right

fullscreen = Fullscreen

manga = Manga mode

reading-direction = Toggle reading direction

slideshow = Slideshow

settings = Settings

shortcut-previous = Previous image (follow reading direction)

shortcut-next = Next image (follow reading direction)

shortcut-advance = Advance (regardless of reading direction)

boundary-ask = Ask what to do

boundary-wrap = Wrap to the other end

boundary-folder = Go to the previous/next folder

boundary-stop = Stop at the boundary

name = Name

modified = Modified date

file-size = File size

extension = Extension

gpu-linear = GPU Linear (fast)

nearest = Nearest (preserve pixels)

bilinear = Bilinear

bicubic = Bicubic

color-dark = Dark (#181A1F)

color-black = Black (#000000)

color-gray = Gray (#404040)

color-white = White (#FFFFFF)

tab-navigation = Navigation & sorting

tab-display = Display & manga

tab-slideshow = Loading & slideshow

tab-keyboard = Keyboard

field-boundary = At the first or last image

field-previous-first = Open first image with Previous folder

field-parent-first = Open parent folders at the first image

field-sort = Sort images by

field-reverse = Reverse sort order

field-natural = Sort numbers in names naturally

field-skip = Images to skip

field-fit = Fit images when opened

field-upscale = Enlarge small images when fitting

field-lock-zoom = Keep zoom when browsing

field-background = Background color

field-manga = Manga mode (two-page spreads)

field-rtl = Read manga from right to left

field-landscape = Show landscape images alone

field-reverse-controls = Reverse left/right keys for right-to-left manga

field-interval = Slideshow interval (seconds)

field-prefetch = Prefetch nearby images

field-wheel = Wheel zooms (Ctrl to browse)

field-shrink = Downscaling filter

field-enlarge = Upscaling filter

enabled = Enabled

disabled = Disabled

press-key = Press a key…

unassigned = Unassigned

binding-help = Select an action, press Space, then press a key. Existing bindings are replaced. Esc cancels key capture.

settings-help = 1–4: tabs · ↑↓: items · Space: edit · List: Enter to select, Esc to cancel · Enter: save

reset-defaults = Restore defaults

cancel = Cancel

save-apply = Save and apply

close = Close

loader-stopped = The image loading worker stopped

resampler-stopped = The resampling worker stopped

path-copied = Path copied

image-copied = Image copied

no-open-image = No image is open

loading-info = Loading information…

loading = Loading…

status-open-hint = Ctrl+O to open an image · Drop a file or folder

resampling-fallback = { "    " }Filter: GPU (size limit)

resampling-pending = { "    " }Resampling…

empty-folder = No images in this folder

empty-hint = Open an image or drop it here

key-hint = ← → Browse    + − Zoom    F Fit    1 Actual size    R Rotate

open = Open

parent = Parent

previous-folder-short = Prev folder

next-folder-short = Next folder

fit = Fit

stop = Stop

slideshow-short = Slideshow

last-image-prompt = Last image. Choose what to do next.

first-image-prompt = First image. Choose what to do next.

go-next-folder = Next folder

go-previous-folder = Previous folder

wrap-first = Back to first

wrap-last = Back to last

settings-path-missing = Cannot determine the settings directory

settings-path-invalid = Invalid settings path

frame-missing = Image frame is missing

image-size-invalid = Invalid image dimensions

spread-data-missing = Page image data is missing

image-data-missing = Image data is missing

no-frames = The image has no frames

gif-limit = GIF exceeds the decoded size limit (128 MiB / 1,000 frames)

image-limit = Decoded image exceeds 128 MiB

heif-limit = HEIF image exceeds the decoded size limit

rgba-plane-missing = RGBA plane is missing

image-too-large = Image is too large

save-extension = Choose an output extension: PNG/JPEG/GIF/BMP/WebP/TIFF/ICO

save-unsupported = Unsupported output format

save-path-invalid = Invalid output path

clipboard-input-failed = Cannot open clipboard input

supported-formats = Supported formats: PNG / JPEG / GIF / BMP / WebP / TIFF / ICO / AVIF / HEIF / HEIC

parent-missing = No parent folder

destination-missing = No destination folder

search-limit = Folder search limit reached

lanczos3 = Lanczos3

cli-help = Nagame
    Usage: nagame [IMAGE_OR_FOLDER]

    Ctrl+O: open | Left/Right: browse | +/-: zoom | F: fit | 1: actual size
    R: rotate | F11: fullscreen | S: slideshow | Esc: stop / leave fullscreen
    Drag: pan | Wheel: browse | Ctrl+Wheel: zoom
    Up/Down: folders | Ctrl+Left/Right: siblings | Alt+Up: parent
    PageUp/PageDown, Home/End: scroll (Shift: horizontal)

current-color = Current color (#{ $color })

settings-row-count = { $count ->
        [one] One item · Scroll with the bar, wheel, or ↑↓
       *[other] { $count } items · Scroll with the bar, wheel, or ↑↓
    }

settings-read-error = Cannot read settings: { $error }

settings-save-error = Cannot save settings: { $error }

error-status = Error: { $error }

settings-error-status = Settings: { $error }

resampling-error = { "    " }Filter: { $error }

window-open-error = Cannot open window: { $error }

window-state-save-error = Cannot save window state: { $error }

window-state-restore-error = Cannot restore window position: { $error }

clipboard-tool-error = Image copying requires { $tool }: { $error }

clipboard-write-error = Cannot write to clipboard: { $error }

clipboard-copy-error = Image copy failed: { $error }

saved-image = Saved: { $path }

image-information = File: { $path }

    Format: { $format }
    File size: { $bytes } bytes
    Original image: { $original_width } × { $original_height }
    Displayed image: { $width } × { $height }
    Frames: { $frames }
    Pages displayed: { $pages }

    Export includes rotation and two-page spreads. Only GIF preserves all frames.

image-status = { $mode ->
        [fit] { $page_range } / { $total }    { $width } × { $height }    { $percent }% · Fit    { $filename }
        [slideshow] { $page_range } / { $total }    { $width } × { $height }    { $percent }%    { $filename }    ▶ Slideshow
        [fit-slideshow] { $page_range } / { $total }    { $width } × { $height }    { $percent }% · Fit    { $filename }    ▶ Slideshow
       *[normal] { $page_range } / { $total }    { $width } × { $height }    { $percent }%    { $filename }
    }


field-language = Language
language-ja = 日本語
language-en = English
language-system = System default

window-state-x11-not-found = Nagame X11 window was not found
window-state-directory-missing = Window state directory is missing
