#include <backbeat.h>
#include <sqlite3.h>

#include <cstddef>
#include <cstdlib>
#include <cstring>

static std::size_t collection_item_count(const bkb_pack *pack,
                                         const bkb_course *course,
                                         const bkb_table *table) {
  return (pack == nullptr ? 0 : pack->bundles_len) +
         (course == nullptr ? 0 : course->charts_len) +
         (table == nullptr ? 0 : table->levels_len);
}

static std::size_t bb_payload_size(const bkb_bb *bb) {
  return bb == nullptr ? 0 : bb->filename.len + bb->chart_len;
}

int main() {
  const char *message = bkb_error_string(BKB_OK);
  const bool linked = std::strcmp(message, "success") == 0;
  const bool fields_accessible =
      collection_item_count(nullptr, nullptr, nullptr) == 0 &&
      bb_payload_size(nullptr) == 0;
  const bool sqlite_compatible = sqlite3_libversion_number() >= 3038000 &&
                                 sqlite3_threadsafe() != 0 &&
                                 sqlite3_compileoption_used("ENABLE_FTS5") != 0;
  return linked && fields_accessible && sqlite_compatible ? EXIT_SUCCESS
                                                        : EXIT_FAILURE;
}
