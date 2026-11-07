//! テストと実ウィンドウテスト用の合成PDF。外部の資料を使わずに、ページの寸法・回転・
//! CropBox・図形・画像・文字（不可視を含む）・暗号化の辞書を組み立てる。

/// 1ページの内容。座標はPDFのユーザー空間（pt、左下が原点）。
#[derive(Clone, Default)]
pub struct Page {
    /// MediaBoxの幅と高さ。
    pub size: [f32; 2],
    /// /Rotate（90の倍数）。
    pub rotate: i32,
    /// CropBox（左, 下, 右, 上）。
    pub crop: Option<[f32; 4]>,
    pub items: Vec<Item>,
}

#[derive(Clone)]
pub enum Item {
    /// 塗りつぶした長方形（左, 下, 幅, 高さ）。
    Rect { rgb: [u8; 3], rect: [f32; 4] },
    /// 無圧縮のRGB画像を長方形へ配置する。左半分がleft、右半分がrightの色。
    Image {
        pixels: [u32; 2],
        left: [u8; 3],
        right: [u8; 3],
        rect: [f32; 4],
    },
    /// Imageと同じ画像を、単位正方形のフォームXObjectに包んで長方形へ配置する
    /// （スキャンをフォームで包むPDFと同じ形）。
    FormImage {
        pixels: [u32; 2],
        color: [u8; 3],
        rect: [f32; 4],
    },
    /// Helveticaの文字列。invisibleなら描画モード3（OCRの文字層と同じ）。
    Text {
        text: String,
        position: [f32; 2],
        size: f32,
        invisible: bool,
    },
}

/// pagesのPDF。encryptedなら、空の利用者パスワードと一致しない暗号化の辞書を付ける。
pub fn pdf(pages: &[Page], encrypted: bool) -> Vec<u8> {
    let mut objects: Vec<Vec<u8>> = Vec::new();
    // 1: Catalog、2: Pages、3: Font。ページごとにPage・Contents・画像を続ける。
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    objects.push(Vec::new());
    objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
    let mut kids = Vec::new();
    for page in pages {
        let page_id = objects.len() + 1;
        let contents_id = page_id + 1;
        objects.push(Vec::new());
        objects.push(Vec::new());
        let mut content = String::new();
        let mut resources = String::new();
        for (index, item) in page.items.iter().enumerate() {
            match item {
                Item::Rect { rgb, rect } => {
                    content += &format!(
                        "{} {} {} rg {} {} {} {} re f\n",
                        f32::from(rgb[0]) / 255.,
                        f32::from(rgb[1]) / 255.,
                        f32::from(rgb[2]) / 255.,
                        rect[0],
                        rect[1],
                        rect[2],
                        rect[3]
                    );
                }
                Item::Image {
                    pixels,
                    left,
                    right,
                    rect,
                } => {
                    let [width, height] = *pixels;
                    let mut data = Vec::with_capacity((width * height * 3) as usize);
                    for _ in 0..height {
                        for x in 0..width {
                            data.extend_from_slice(if x < width / 2 { left } else { right });
                        }
                    }
                    let image_id = objects.len() + 1;
                    let mut object = format!(
                        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
                         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
                        data.len()
                    )
                    .into_bytes();
                    object.extend_from_slice(&data);
                    object.extend_from_slice(b"\nendstream");
                    objects.push(object);
                    resources += &format!("/Im{index} {image_id} 0 R ");
                    content += &format!(
                        "q {} 0 0 {} {} {} cm /Im{index} Do Q\n",
                        rect[2], rect[3], rect[0], rect[1]
                    );
                }
                Item::FormImage {
                    pixels,
                    color,
                    rect,
                } => {
                    let [width, height] = *pixels;
                    let data: Vec<u8> = color
                        .iter()
                        .copied()
                        .cycle()
                        .take((width * height * 3) as usize)
                        .collect();
                    let image_id = objects.len() + 1;
                    let mut image = format!(
                        "<< /Type /XObject /Subtype /Image /Width {width} /Height {height} \
                         /ColorSpace /DeviceRGB /BitsPerComponent 8 /Length {} >>\nstream\n",
                        data.len()
                    )
                    .into_bytes();
                    image.extend_from_slice(&data);
                    image.extend_from_slice(b"\nendstream");
                    objects.push(image);
                    let form_content = "q 1 0 0 1 0 0 cm /Im0 Do Q";
                    objects.push(
                        format!(
                            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] \
                             /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Length {} >>\n\
                             stream\n{form_content}\nendstream",
                            form_content.len()
                        )
                        .into_bytes(),
                    );
                    resources += &format!("/Fm{index} {} 0 R ", objects.len());
                    content += &format!(
                        "q {} 0 0 {} {} {} cm /Fm{index} Do Q\n",
                        rect[2], rect[3], rect[0], rect[1]
                    );
                }
                Item::Text {
                    text,
                    position,
                    size,
                    invisible,
                } => {
                    content += &format!(
                        "BT /F1 {size} Tf {} Tr {} {} Td ({text}) Tj ET\n",
                        if *invisible { 3 } else { 0 },
                        position[0],
                        position[1]
                    );
                }
            }
        }
        let crop = page.crop.map_or(String::new(), |c| {
            format!("/CropBox [{} {} {} {}] ", c[0], c[1], c[2], c[3])
        });
        objects[page_id - 1] = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] {crop}/Rotate {} \
             /Resources << /Font << /F1 3 0 R >> /XObject << {resources}>> >> \
             /Contents {contents_id} 0 R >>",
            page.size[0], page.size[1], page.rotate
        )
        .into_bytes();
        let mut stream = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
        stream.extend_from_slice(content.as_bytes());
        stream.extend_from_slice(b"\nendstream");
        objects[contents_id - 1] = stream;
        kids.push(format!("{page_id} 0 R"));
    }
    objects[1] = format!(
        "<< /Type /Pages /Kids [{}] /Count {} >>",
        kids.join(" "),
        pages.len()
    )
    .into_bytes();
    let encrypt_id = encrypted.then(|| {
        objects.push(
            format!(
                "<< /Filter /Standard /V 1 /R 2 /Length 40 /P -4 /O <{}> /U <{}> >>",
                "11".repeat(32),
                "22".repeat(32)
            )
            .into_bytes(),
        );
        objects.len()
    });
    let mut output = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        output.extend_from_slice(object);
        output.extend_from_slice(b"\nendobj\n");
    }
    let xref = output.len();
    output.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        output.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    let encrypt = encrypt_id.map_or(String::new(), |id| {
        format!("/Encrypt {id} 0 R /ID [<{0}> <{0}>] ", "33".repeat(16))
    });
    output.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R {encrypt}>>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    output
}
