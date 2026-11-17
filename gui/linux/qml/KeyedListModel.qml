import QtQuick

// A list model updated in place from a JS array, matched by key, so a view keeps its scroll position
// and delegates across updates (assigning a new array to a view's model resets both). Each row holds
// the item's `key` and its `json`; a delegate parses `json`.
ListModel {
    function sync(items, keyOf) {
        const keys = items.map(item => String(keyOf(item)))
        const wanted = new Set(keys)
        for (let i = count - 1; i >= 0; i--) {
            if (!wanted.has(get(i).key))
                remove(i)
        }
        items.forEach((item, i) => {
            const json = JSON.stringify(item)
            let at = i
            while (at < count && get(at).key !== keys[i])
                at++
            if (at === count) {
                insert(i, { key: keys[i], json: json })
                return
            }
            if (at !== i)
                move(at, i, 1)
            if (get(i).json !== json)
                setProperty(i, "json", json)
        })
        while (count > items.length)
            remove(count - 1)
    }
}
