use super::*;

fn database() -> MimeDatabase {
    let mut database = MimeDatabase::default();
    database.add_types(
        "image/jpeg\nimage/heif\ntext/vcard\ntext/plain\naudio/AMR\nvideo/3gpp\nvideo/mp4\napplication/xml\n\
             application/x-test+xml\napplication/pdf\napplication/vnd.ms-excel\napplication/zip\napplication/rtf\n\
             application/smil+xml\napplication/vnd.apple.keynote\napplication/vnd.oasis.opendocument.text\n\
             application/vnd.oasis.opendocument.presentation\napplication/vnd.oasis.opendocument.spreadsheet\n\
             application/vnd.ms-word.document.macroEnabled.12\n\
             application/vnd.openxmlformats-officedocument.wordprocessingml.document\n",
    );
    database.add_aliases(
        "text/x-vcard text/vcard\nimage/pjpeg image/jpeg\nimage/heic image/heif\ntext/rtf application/rtf\n\
             application/smil application/smil+xml\n\
             application/x-iwork-keynote-sffkey application/vnd.apple.keynote\n",
    );
    database.add_subclasses(
        "text/vcard text/plain\napplication/xml text/plain\napplication/x-test+xml application/xml\n\
             application/smil+xml application/xml\napplication/rtf text/plain\nvideo/3gpp video/mp4\n\
             application/vnd.apple.keynote application/zip\n\
             application/vnd.ms-word.document.macroEnabled.12 \
             application/vnd.openxmlformats-officedocument.wordprocessingml.document\n",
    );
    database.add_globs(
        "# comment\n50:image/jpeg:*.jfif\n50:image/jpeg:*.jpg\n50:text/vcard:*.vcf\n55:audio/AMR:*.amr\n\
             50:application/x-tar:*.tar.gz\n50:image/png:*.png\n10:text/plain:*.png\n",
    );
    database
}

#[test]
fn families_follow_the_media_type_and_sub_classes() {
    let database = database();
    assert_eq!(database.family_of("audio/amr"), FileFamily::Audio);
    assert_eq!(database.family_of("video/3gpp"), FileFamily::Video);
    assert_eq!(database.family_of("text/plain"), FileFamily::Text);
    assert_eq!(database.family_of("application/x-test+xml"), FileFamily::Text);
    assert_eq!(database.family_of("text/x-vCard"), FileFamily::VCard);
    assert_eq!(database.family_of("application/pdf"), FileFamily::Document);
    assert_eq!(database.family_of("application/vnd.ms-excel"), FileFamily::Document);
    assert_eq!(database.family_of("application/zip"), FileFamily::Other);
}

#[test]
fn macos_answers_come_before_the_database() {
    let database = database();
    for undeclared in ["IMAGE/PJPEG", "application/rtf", "application/smil", "application/vnd.apple.keynote"] {
        assert_eq!(database.family_of(undeclared), FileFamily::Other, "{undeclared}");
        assert_eq!(database.extension_of(undeclared), None, "{undeclared}");
    }
    assert_eq!(database.family_of("text/rtf"), FileFamily::Text);
    assert_eq!(database.family_of("application/x-iwork-keynote-sffkey"), FileFamily::Document);
    assert_eq!(database.extension_of("application/x-iwork-keynote-sffkey").as_deref(), Some(".key"));
    assert_eq!(database.extension_of("image/jpeg").as_deref(), Some(".jpeg"));
    assert_eq!(database.extension_of("image/heif").as_deref(), Some(".heif"));
    assert_eq!(database.extension_of("image/heic").as_deref(), Some(".heic"));
    assert_eq!(database.extension_of("text/x-vcard").as_deref(), Some(".vcf"));
}

#[test]
fn macos_aliases_name_the_type_they_alias() {
    let database = database();
    assert_eq!(database.family_of("image/jpg"), FileFamily::Image);
    assert_eq!(database.extension_of("Image/JPG").as_deref(), Some(".jpeg"));
}

#[test]
fn documents_are_what_macos_takes_for_documents() {
    let database = database();
    assert_eq!(database.family_of("application/vnd.oasis.opendocument.text"), FileFamily::Document);
    for other in [
        "application/vnd.oasis.opendocument.presentation",
        "application/vnd.oasis.opendocument.spreadsheet",
        "application/vnd.ms-word.document.macroEnabled.12",
    ] {
        assert_eq!(database.family_of(other), FileFamily::Other, "{other}");
    }
}

#[test]
fn a_type_the_database_does_not_know_is_other() {
    let database = database();
    assert_eq!(database.family_of("image/x-foo"), FileFamily::Other);
    assert_eq!(database.family_of("video/x-unknown"), FileFamily::Other);
    assert_eq!(database.extension_of("image/x-foo"), None);
}

#[test]
fn a_type_with_parameters_or_no_subtype_is_other() {
    let database = database();
    assert_eq!(database.family_of("text/plain; charset=utf-8"), FileFamily::Other);
    assert_eq!(database.family_of("image"), FileFamily::Other);
    assert_eq!(database.family_of(""), FileFamily::Other);
    assert_eq!(database.extension_of("image/jpeg; q=1"), None);
}

#[test]
fn without_a_database_the_media_type_decides() {
    let database = MimeDatabase::default();
    assert_eq!(database.family_of("image/webp"), FileFamily::Image);
    assert_eq!(database.family_of("image/x-foo"), FileFamily::Image);
    assert_eq!(database.family_of("text/x-vcard"), FileFamily::Text);
    assert_eq!(database.family_of("image/pjpeg"), FileFamily::Other, "macOS's answers still hold");
    assert_eq!(database.extension_of("image/webp"), None);
    assert_eq!(database.type_of_file_name("a.png"), None);
}

#[test]
fn file_names_are_typed_by_their_heaviest_glob() {
    let database = database();
    assert_eq!(database.type_of_file_name("IMG_1.JPG").as_deref(), Some("image/jpeg"));
    assert_eq!(database.type_of_file_name("card.vcf").as_deref(), Some("text/vcard"));
    assert_eq!(database.type_of_file_name("x.png").as_deref(), Some("image/png"));
    assert_eq!(database.type_of_file_name("note.amr").as_deref(), Some("audio/amr"));
    assert_eq!(database.type_of_file_name("README"), None);
    assert_eq!(database.type_of_file_name("archive.gz"), None);
}

#[test]
fn the_preferred_extension_is_the_definitions_first_simple_glob() {
    let definition = r#"<mime-type type="image/jpeg"><comment>JPEG</comment><glob pattern="*.jpg"/>
            <glob pattern="*.jpeg"/></mime-type>"#;
    assert_eq!(first_glob_extension(definition).as_deref(), Some("jpg"));
    let tricky = r#"<glob pattern="[Mm]akefile"/><glob weight="60" pattern="*.TXT"/>"#;
    assert_eq!(first_glob_extension(tricky).as_deref(), Some("txt"));
    assert_eq!(first_glob_extension("<mime-type/>"), None);
}

#[test]
fn extensions_come_from_the_definition_files() {
    let dir = std::env::temp_dir().join(format!("phonelink-mime-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("audio")).expect("the test folder is created");
    std::fs::create_dir_all(dir.join("video")).expect("the test folder is created");
    std::fs::write(dir.join("audio/AMR.xml"), r#"<mime-type><glob pattern="*.amr"/></mime-type>"#)
        .expect("the definition is written");
    // Newer databases write the definition of `video/X-Test` as `video/x-test.xml`.
    std::fs::write(dir.join("video/x-test.xml"), r#"<mime-type><glob pattern="*.xtv"/></mime-type>"#)
        .expect("the definition is written");
    std::fs::write(dir.join("types"), "audio/AMR\nvideo/X-Test\n").expect("the types are written");
    let database = MimeDatabase::load(vec![dir.clone()]);
    assert_eq!(database.extension_of("audio/amr").as_deref(), Some(".amr"));
    assert_eq!(database.extension_of("video/x-test").as_deref(), Some(".xtv"));
    assert_eq!(database.extension_of("video/unknown"), None);
    std::fs::remove_dir_all(&dir).ok();
}

/// macOS's answers (`UTType(mimeType:)` on macOS 27) for common attachment types.
const ON_MACOS: [(&str, FileFamily, Option<&str>); 122] = [
    ("image/jpeg", FileFamily::Image, Some(".jpeg")),
    ("image/jpg", FileFamily::Image, Some(".jpeg")),
    ("image/pjpeg", FileFamily::Other, None),
    ("image/png", FileFamily::Image, Some(".png")),
    ("image/x-png", FileFamily::Other, None),
    ("image/gif", FileFamily::Image, Some(".gif")),
    ("image/bmp", FileFamily::Image, Some(".bmp")),
    ("image/x-bmp", FileFamily::Other, None),
    ("image/x-ms-bmp", FileFamily::Other, None),
    ("image/webp", FileFamily::Image, Some(".webp")),
    ("image/tiff", FileFamily::Image, Some(".tiff")),
    ("image/heic", FileFamily::Image, Some(".heic")),
    ("image/heif", FileFamily::Image, Some(".heif")),
    ("image/heic-sequence", FileFamily::Image, Some(".heics")),
    ("image/heif-sequence", FileFamily::Image, Some(".heifs")),
    ("image/avif", FileFamily::Image, Some(".avif")),
    ("image/svg+xml", FileFamily::Image, Some(".svg")),
    ("image/vnd.wap.wbmp", FileFamily::Other, None),
    ("image/x-icon", FileFamily::Other, None),
    ("image/vnd.microsoft.icon", FileFamily::Image, Some(".ico")),
    ("image/jxl", FileFamily::Image, Some(".jxl")),
    ("video/mp4", FileFamily::Video, Some(".mp4")),
    ("video/3gpp", FileFamily::Video, Some(".3gp")),
    ("video/3gpp2", FileFamily::Video, Some(".3g2")),
    ("video/3gp", FileFamily::Other, None),
    ("video/quicktime", FileFamily::Video, Some(".mov")),
    ("video/webm", FileFamily::Video, Some(".webm")),
    ("video/mpeg", FileFamily::Video, Some(".mpg")),
    ("video/x-msvideo", FileFamily::Video, Some(".avi")),
    ("video/avi", FileFamily::Video, Some(".avi")),
    ("video/x-matroska", FileFamily::Other, None),
    ("video/mp4v-es", FileFamily::Video, Some(".mp4")),
    ("video/h264", FileFamily::Other, None),
    ("video/x-flv", FileFamily::Video, Some(".flv")),
    ("video/x-ms-wmv", FileFamily::Video, Some(".wmv")),
    ("video/x-m4v", FileFamily::Video, Some(".m4v")),
    ("video/mp2t", FileFamily::Other, None),
    ("audio/mp4", FileFamily::Audio, Some(".mp4")),
    ("audio/mpeg", FileFamily::Audio, Some(".mp3")),
    ("audio/mp3", FileFamily::Audio, Some(".mp3")),
    ("audio/mpeg3", FileFamily::Audio, Some(".mp3")),
    ("audio/x-mp3", FileFamily::Audio, Some(".mp3")),
    ("audio/x-mpeg", FileFamily::Audio, Some(".mp3")),
    ("audio/amr", FileFamily::Audio, Some(".amr")),
    ("audio/AMR", FileFamily::Audio, Some(".amr")),
    ("audio/amr-wb", FileFamily::Other, None),
    ("audio/3gpp", FileFamily::Video, Some(".3gp")),
    ("audio/3gpp2", FileFamily::Video, Some(".3g2")),
    ("audio/aac", FileFamily::Audio, Some(".aac")),
    ("audio/x-aac", FileFamily::Audio, Some(".aac")),
    ("audio/aacp", FileFamily::Other, None),
    ("audio/midi", FileFamily::Audio, Some(".midi")),
    ("audio/mid", FileFamily::Other, None),
    ("audio/x-midi", FileFamily::Audio, Some(".midi")),
    ("audio/sp-midi", FileFamily::Other, None),
    ("audio/imelody", FileFamily::Other, None),
    ("audio/x-imelody", FileFamily::Other, None),
    ("audio/ogg", FileFamily::Audio, Some(".ogg")),
    ("audio/x-wav", FileFamily::Audio, Some(".wav")),
    ("audio/wav", FileFamily::Audio, Some(".wav")),
    ("audio/vnd.wave", FileFamily::Audio, Some(".wav")),
    ("audio/flac", FileFamily::Audio, Some(".flac")),
    ("audio/x-flac", FileFamily::Other, None),
    ("audio/webm", FileFamily::Video, Some(".webm")),
    ("audio/x-m4a", FileFamily::Audio, Some(".m4a")),
    ("audio/m4a", FileFamily::Other, None),
    ("audio/opus", FileFamily::Other, None),
    ("audio/x-ms-wma", FileFamily::Other, None),
    ("audio/qcelp", FileFamily::Other, None),
    ("audio/evrc", FileFamily::Other, None),
    ("audio/mobile-xmf", FileFamily::Other, None),
    ("audio/x-matroska", FileFamily::Other, None),
    ("audio/basic", FileFamily::Audio, Some(".au")),
    ("audio/x-aiff", FileFamily::Audio, Some(".aiff")),
    ("audio/aiff", FileFamily::Audio, Some(".aiff")),
    ("text/vcard", FileFamily::VCard, Some(".vcf")),
    ("text/x-vcard", FileFamily::VCard, Some(".vcf")),
    ("text/directory", FileFamily::VCard, Some(".vcf")),
    ("text/calendar", FileFamily::Text, Some(".ics")),
    ("text/x-vcalendar", FileFamily::Text, Some(".vcs")),
    ("text/plain", FileFamily::Text, Some(".txt")),
    ("text/html", FileFamily::Text, Some(".html")),
    ("text/rtf", FileFamily::Text, Some(".rtf")),
    ("text/csv", FileFamily::Text, Some(".csv")),
    ("text/x-vnote", FileFamily::Other, None),
    ("text/x-vtodo", FileFamily::Other, None),
    ("text/markdown", FileFamily::Text, Some(".md")),
    ("text/xml", FileFamily::Text, Some(".xml")),
    ("application/pdf", FileFamily::Document, Some(".pdf")),
    ("application/rtf", FileFamily::Other, None),
    ("application/smil", FileFamily::Other, None),
    ("application/smil+xml", FileFamily::Other, None),
    ("application/zip", FileFamily::Other, Some(".zip")),
    ("application/octet-stream", FileFamily::Other, None),
    ("application/vnd.android.package-archive", FileFamily::Other, Some(".apk")),
    ("application/ogg", FileFamily::Other, None),
    ("application/json", FileFamily::Text, Some(".json")),
    ("application/xml", FileFamily::Text, Some(".xml")),
    ("application/x-iwork-keynote-sffkey", FileFamily::Document, Some(".key")),
    ("application/vnd.apple.keynote", FileFamily::Other, None),
    ("application/vnd.apple.numbers", FileFamily::Other, None),
    ("application/x-iwork-numbers-sffnumbers", FileFamily::Document, Some(".numbers")),
    ("application/vnd.apple.pages", FileFamily::Other, None),
    ("application/x-iwork-pages-sffpages", FileFamily::Other, Some(".pages")),
    ("application/vnd.oasis.opendocument.text", FileFamily::Document, Some(".odt")),
    ("application/vnd.oasis.opendocument.presentation", FileFamily::Other, Some(".odp")),
    ("application/vnd.oasis.opendocument.spreadsheet", FileFamily::Other, Some(".ods")),
    ("application/msword", FileFamily::Document, Some(".doc")),
    ("application/vnd.ms-excel", FileFamily::Document, Some(".xls")),
    ("application/vnd.ms-powerpoint", FileFamily::Document, Some(".ppt")),
    ("application/vnd.openxmlformats-officedocument.wordprocessingml.document", FileFamily::Document, Some(".docx")),
    ("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", FileFamily::Document, Some(".xlsx")),
    ("application/vnd.openxmlformats-officedocument.presentationml.presentation", FileFamily::Document, Some(".pptx")),
    ("application/vnd.openxmlformats-officedocument.presentationml.slideshow", FileFamily::Document, Some(".ppsx")),
    ("application/vnd.ms-excel.sheet.macroenabled.12", FileFamily::Document, Some(".xlsm")),
    ("application/vnd.ms-word.document.macroenabled.12", FileFamily::Other, Some(".docm")),
    ("application/x-pdf", FileFamily::Other, None),
    ("application/vnd.ms-word", FileFamily::Other, None),
    ("application/x-msword", FileFamily::Other, None),
    ("application/ics", FileFamily::Other, None),
    ("application/x-vcard", FileFamily::Other, None),
    ("application/vcard", FileFamily::Other, None),
];

#[test]
#[ignore = "reads the system's shared MIME-info database at /usr/share/mime"]
fn common_types_answer_as_on_macos() {
    let database = MimeDatabase::load(vec![PathBuf::from("/usr/share/mime")]);
    assert!(database.is_loaded(), "the database is installed");
    let differing: Vec<String> = ON_MACOS
        .iter()
        .filter_map(|(mime, family, extension)| {
            let got = (database.family_of(mime), database.extension_of(mime));
            (got != (*family, extension.map(str::to_owned)))
                .then(|| format!("{mime}: {got:?}, on macOS ({family:?}, {extension:?})"))
        })
        .collect();
    assert!(differing.is_empty(), "{differing:#?}");
}
