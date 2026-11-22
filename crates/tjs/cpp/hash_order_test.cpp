// The TJS2 hash table's additional order list.
//
// The table keeps an additional order list that holds its live elements, and
// a count that is the number of elements in that list. The delete path takes
// the element out of the list and decrements the count together; when the
// count falls out of step with the list, the delete path has to repair the
// bookkeeping from the table's live set instead of emptying the list. A list
// that is emptied while the table still holds live elements orphans them:
// they stay in the table, they are no longer in the list, and the next
// delete of one of them walks off the list.
//
// This harness drives that: it puts the bookkeeping out of step (two live
// elements fall out of the order list while the count stays in step with
// the list), deletes the elements, and checks that the bookkeeping is
// consistent again once the count has reached zero.
//
// The table's bookkeeping is private, so the harness reaches it with the
// `private public` trick and the vendored header carries no test scaffolding.

#include "tjsCommHead.h"

// `tjsCommHead.h` pulls in `tjs.h`, so the trick below applies to the hash
// table's own header only.
#define private public
#include "tjsHashSearch.h"
#undef private

namespace {
    typedef TJS::tTJSHashTable<void *, tjs_int32> Table;

    // The number of elements in the additional order list.
    tjs_uint OrderListLength(Table &table) {
        tjs_uint length = 0;
        for(Table::tIterator it = table.GetFirst(); !it.IsNull(); ++it)
            length++;
        return length;
    }

    // Whether every element the table still holds is in the order list.
    bool EveryLiveElementIsInTheOrderList(Table &table,
                                           void *const *keys,
                                           tjs_uint keyCount) {
        for(tjs_uint i = 0; i < keyCount; i++) {
            void *const *foundKey = nullptr;
            tjs_int32 *foundValue = nullptr;
            if(!table.Find(keys[i], foundKey, foundValue))
                continue; // the element is not in the table any more
            bool found = false;
            for(Table::tIterator it = table.GetFirst(); !it.IsNull(); ++it) {
                if(it.GetKey() == keys[i]) {
                    found = true;
                    break;
                }
            }
            if(!found)
                return false;
        }
        return true;
    }
} // namespace

extern "C" {
// Checks the delete path of the hash table's additional order list.
//
// The table's bookkeeping is put out of step - two live elements fall out
// of the order list while the count stays in step with the list - and the
// elements are then deleted. The count reaches zero at the first delete,
// and the delete path must have rebuilt the list from the table's live set
// by then.
//
// Returns an empty string when the bookkeeping is consistent, and a message
// naming the first broken invariant otherwise.
const char *tjs2_test_hash_order_repair() {
    Table table;
    void *keys[3] = {(void *)0x1000, (void *)0x2000, (void *)0x3000};
    for(tjs_int i = 0; i < 3; i++)
        table.Add(keys[i], i);

    if(table.Count != 3 || OrderListLength(table) != 3)
        return "the order list does not hold the three added elements";

    // The divergence: the first two elements of the order list fall out of
    // it while the count stays in step with the list. The table holds three
    // live elements, the list holds one.
    for(int i = 0; i < 2; i++) {
        if(!table.NFirst)
            break;
        table.NFirst = table.NFirst->NNext;
        if(table.NFirst)
            table.NFirst->NPrev = nullptr;
        table.Count--;
    }

    // The first delete brings the count to zero, and the delete path must
    // rebuild the list from the table's live set: the two elements that
    // fell out of the list are live elements of the table, so the list
    // holds them again.
    if(!table.Delete(keys[0]))
        return "the first element could not be deleted";
    if(table.Count != 2 || OrderListLength(table) != 2)
        return "the order list and the count disagree after the count "
               "reached zero";
    if(!EveryLiveElementIsInTheOrderList(table, keys, 3))
        return "a live element is not in the order list after the count "
               "reached zero";

    if(!table.Delete(keys[1]))
        return "the second element could not be deleted";
    if(table.Count != 1 || OrderListLength(table) != 1)
        return "the order list and the count disagree after the second "
               "delete";

    if(!table.Delete(keys[2]))
        return "the third element could not be deleted";
    if(table.Count != 0 || OrderListLength(table) != 0)
        return "the order list is not empty after the last delete";

    return "";
}
} // extern "C"
