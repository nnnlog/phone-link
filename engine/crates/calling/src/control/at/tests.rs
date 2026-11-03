use super::*;

#[test]
fn finals_and_ring() {
    assert_eq!(parse("OK"), Line::Final(Final::Ok));
    assert_eq!(parse("ERROR"), Line::Final(Final::Error));
    assert_eq!(parse("+CME ERROR: 30"), Line::Final(Final::CmeError(30)));
    assert_eq!(parse("NO CARRIER"), Line::Final(Final::NoCarrier));
    assert_eq!(parse("RING"), Line::Ring);
}

#[test]
fn slc_answers() {
    assert_eq!(parse("+BRSF: 4079"), Line::Brsf(0xFEF));
    assert_eq!(
        parse(
            r#"+CIND: ("call",(0,1)),("callsetup",(0-3)),("service",(0-1)),("signal",(0-5)),("roam",(0,1)),("battchg",(0-5)),("callheld",(0-2))"#
        ),
        Line::CindNames(
            ["call", "callsetup", "service", "signal", "roam", "battchg", "callheld"].map(String::from).to_vec()
        )
    );
    assert_eq!(parse("+CIND: 0,0,1,4,0,3,0"), Line::CindValues(vec![0, 0, 1, 4, 0, 3, 0]));
    assert_eq!(
        parse("+CHLD: (0,1,1x,2,2x,3,4)"),
        Line::Chld(["0", "1", "1x", "2", "2x", "3", "4"].map(String::from).to_vec())
    );
}

#[test]
fn clcc_lines() {
    assert_eq!(
        parse(r#"+CLCC: 1,1,4,0,0,"+15550100",145"#),
        Line::Clcc(ClccEntry { index: 1, incoming: true, status: 4, number: Some("+15550100".into()) })
    );
    assert_eq!(parse("+CLCC: 2,0,2,0,0"), Line::Clcc(ClccEntry { index: 2, incoming: false, status: 2, number: None }));
    assert_eq!(
        parse(r#"+CLCC: 3,1,0,0,0,"",128"#),
        Line::Clcc(ClccEntry { index: 3, incoming: true, status: 0, number: None })
    );
    assert_eq!(parse("+CLCC: 1,1"), Line::Other("+CLCC: 1,1".into()));
}

#[test]
fn unsolicited() {
    assert_eq!(parse("+CIEV: 2,1"), Line::Ciev { index: 2, value: 1 });
    assert_eq!(parse(r#"+CLIP: "+15550100",145,,,"Ada""#), Line::Clip(Some("+15550100".into())));
    assert_eq!(parse(r#"+CLIP: "",128"#), Line::Clip(None));
    assert_eq!(parse(r#"+CCWA: "5550101",129,1"#), Line::Ccwa(Some("5550101".into())));
    assert_eq!(parse("+BSIR: 0"), Line::Other("+BSIR: 0".into()));
    assert_eq!(parse("+CNMI: (3),(1),(0),(0),(0)"), Line::Other("+CNMI: (3),(1),(0),(0),(0)".into()));
    assert_eq!(parse("+CIEV: x,1"), Line::Other("+CIEV: x,1".into()));
}

#[test]
fn splitter_joins_pieces_and_drops_empty_lines() {
    let mut s = LineSplitter::default();
    assert_eq!(s.push(b"\r\n+BRS"), Vec::<String>::new());
    assert_eq!(s.push(b"F: 4079\r\n\r\nOK\r\n"), vec!["+BRSF: 4079".to_string(), "OK".to_string()]);
    assert_eq!(s.push(b"\r\nRING\r"), vec!["RING".to_string()]);
}

#[test]
fn commands() {
    assert_eq!(command::brsf(), "AT+BRSF=102");
    assert_eq!(command::dial("+15550100"), "ATD+15550100;");
}

#[test]
fn log_names_carry_no_values() {
    assert_eq!(command_name("ATD+15550100;"), "ATD");
    assert_eq!(command_name("AT+CHLD=2"), "AT+CHLD");
    assert_eq!(command_name("AT+CIND?"), "AT+CIND");
    assert_eq!(command_name("ATA"), "ATA");
    assert_eq!(line_name("+CNUM: ,\"+15550100\",145"), "+CNUM");
    assert_eq!(line_name("whatever 5550100"), "other");
}
