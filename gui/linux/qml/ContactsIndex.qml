import QtQuick

// The contacts list's grouping, order and search. A contact goes under its shown name's first
// character: a Latin letter without its diacritics, a Hangul initial consonant (a tense one under its
// plain one), else "#"; the groups run A–Z, ㄱ–ㅎ, #, and within a group contacts sort by name, then by
// contact id.
QtObject {
    readonly property var latin: "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("")
    readonly property var hangul: ["ㄱ", "ㄴ", "ㄷ", "ㄹ", "ㅁ", "ㅂ", "ㅅ", "ㅇ", "ㅈ", "ㅊ", "ㅋ", "ㅌ", "ㅍ", "ㅎ"]
    // The 19 initial consonants of a Hangul syllable, in Unicode order, each as its plain consonant.
    readonly property var initials: ["ㄱ", "ㄱ", "ㄴ", "ㄷ", "ㄷ", "ㄹ", "ㅁ", "ㅂ", "ㅂ", "ㅅ", "ㅅ", "ㅇ", "ㅈ", "ㅈ",
                                     "ㅊ", "ㅋ", "ㅌ", "ㅍ", "ㅎ"]
    // Compatibility jamo that are tense consonants, by their plain one.
    readonly property var tense: ({ "ㄲ": "ㄱ", "ㄸ": "ㄷ", "ㅃ": "ㅂ", "ㅆ": "ㅅ", "ㅉ": "ㅈ" })
    readonly property var order: latin.concat(hangul, ["#"])

    // `text` without case, diacritics or width differences, for comparing.
    function fold(text) {
        return text.normalize("NFKD").replace(/[\u0300-\u036f]/g, "").normalize("NFC").toLocaleLowerCase()
    }

    // The group a shown name goes under.
    function group(name) {
        const trimmed = name.trim()
        if (trimmed.length === 0)
            return "#"
        const code = trimmed.codePointAt(0)
        if (code >= 0xAC00 && code <= 0xD7A3)
            return initials[Math.floor((code - 0xAC00) / 588)]
        if (code >= 0x1100 && code <= 0x1112)
            return initials[code - 0x1100]
        const first = String.fromCodePoint(code)
        if (hangul.includes(first))
            return first
        if (tense[first] !== undefined)
            return tense[first]
        const letter = first.normalize("NFKD").replace(/[\u0300-\u036f]/g, "").toUpperCase().charAt(0)
        return latin.includes(letter) ? letter : "#"
    }

    // Every white-space-separated word of `query` is found, ignoring case and diacritics, in the name
    // or one of the search terms; a word of digits alone also matches a term whose digits contain it
    // ("0101234" finds "010-1234-5678").
    function matches(contact, query) {
        const fields = [contact.name].concat(contact.terms).map(fold)
        return query.split(/\s+/).filter(word => word.length > 0).every(word => {
            const folded = fold(word)
            if (fields.some(field => field.includes(folded)))
                return true
            return /^[0-9]+$/.test(word) && contact.terms.some(term => term.replace(/[^0-9]/g, "").includes(word))
        })
    }

    // The list's rows for the contacts that match `query`: each group's header, then its contacts.
    function rows(contacts, query) {
        const shown = query.length === 0 ? contacts : contacts.filter(contact => matches(contact, query))
        const groups = {}
        shown.forEach(contact => {
            const title = group(contact.name)
            if (groups[title] === undefined)
                groups[title] = []
            groups[title].push(contact)
        })
        const result = []
        order.forEach(title => {
            const members = groups[title]
            if (members === undefined)
                return
            members.sort((a, b) => a.name.localeCompare(b.name) || a.id - b.id)
            result.push({ header: title, key: "header:" + title })
            members.forEach(contact => result.push({ contact: contact, key: "contact:" + contact.id }))
        })
        return result
    }

    // `text` percent-encoded but for the unreserved characters (A–Z, a–z, 0–9, "-", ".", "_", "~").
    function escaped(text) {
        return encodeURIComponent(text).replace(/[!'()*]/g, c => "%" + c.charCodeAt(0).toString(16).toUpperCase())
    }

    // A contact's link: as given when it names a scheme, else as an `http://` link.
    function link(text) {
        const trimmed = text.trim()
        return /^[A-Za-z][A-Za-z0-9+.-]*:/.test(trimmed) ? trimmed : "http://" + trimmed
    }
}
