# Tantivy index file formats
This document defines the index file formats used in Tantivy. 

Tantivy has one metadata json file for each index and a bunch of files for each segment, each file storing a specific datastructure.

## Index metadata
[Source](../../src/core/index_meta.rs)

For a given index, Tantivy stores the following metadata in a json file `meta.json` :
- the list of segments with its metadata id, max_doc, deletes
- the index schema represented by the list of fields with name, type and option data
- the opstamp or timestamp of the operation (commit)

```
{
  "segments": [
    {
      "segment_id": uuid string,
      "max_doc": int|null,
      "deletes": int|null
    },
    ...
  ],
  "schema": [
    {
      "name": string,
      "type": string,
      "options": {
        "indexing": {
          "record": string,
          "tokenizer": string
        },
        "stored": bool
      }
    },
 ],
 ...
 "opstamp": int
}
```

## Summary of segment file extensions
For a given segment, Tantivy stores a bunch of files whose name is set by segment uuid and whose extension defines the datastructure.

| Name | Extension | Description |
| --- | --- | --- |
| Posting list | `.idx` | List of sorted doc ids associated to each term |
| Term dictionnary | `.term` | Dictionary associating `Term`s to an address into the `postings` file and the `positions` file |
| Term positions | `.pos` | Positions of terms in each document |
| Document store | `.store` | Row-oriented, compressed storage of the documents |
| Fast fields | `.fast` | Column-oriented random-access storage of fields |
| Fieldnorm | `.fieldnorm` | Stores the sum  of the length (in terms) of each field for each document ? |
| Tombstone | `.del` | Bitset describing which document of the segment is deleted  |


### Endianness
By default integers and floats are serialized with little endian order. In some specific cases, Tantivy uses big indian, the documentation will explicitely indicates when big endian is used.


### Composite file structure
[Source](../../src/common/composite_file.rs)

All segment files needs to store data for each field except for tombstone file (.del). In this case, a footer is added which stores for each field an offset that indicates the starting point (or file address) of its data.

```
Footer --> {{field_offset, field_file_address}^num_field, num_field, footer_len}
field_offset --> VInt
field_file_address --> {field_id, idx}
field_id --> u32
idx --> VInt
num_field --> VInt
footer_len --> u32
```


### Posting list
[Source](../../src/postings/serializer.rs)

Posting list file (.idx) stores data for each field (it's a [composite file](#composite-file-structure)) and thus has the dedicated footer to get data for each field. The following data structure is repeated for each field, we omit that repetition for clarity.

Posting list (.idx) is divided into 2 parts:
- skip list and meta data used to decompress
- posting list

```
skip list --> {last_doc_id_encoded, decompress_doc_id_num_bits, decompress_termfreq_num_bits, total_term_freq, fieldnorm_id, block_wand_max}^num_doc/128
last_doc_id_encoded --> u32
decompress_doc_id_num_bits --> u8
decompress_termfreq_num_bits --> u8, present only if field index record option has freq
total_term_freq --> u32, present only if field index record option has positions
fieldnorm_id --> u8
block_wand_max --> u8
```

```
posting list --> {bitpacked_doc_ids, bitpacked_term_freq}^{num_doc/128}{{vintencoded_doc_ids, vintencoded_term_freq}^{num_doc % 128}}
bitpacked_doc_ids --> bitpacked delta encoded of 128 doc_id
bitpacked_term_freq --> bitpacked term frequency of 128 term_freq, present only if field index record option has freq
vintencoded_doc_ids --> variable int encoded and delta encoded of num_doc % 128 doc_ids
vintencoded_term_freq --> variable int encoded num_doc % 128 term_freq, present only if field index record option has freq
```

### Term dictionnary

```
Term dictionnary file (.term) --> {fst_index, term_info_store, footer_len_bytes}
fst_index --> finite state transducer data structure
term_info_store --> term store data structure
footer_len_bytes --> u64
```

#### Finiste state transducer

#### Term store
```
Term store --> {len, num_terms, block_meta_file, term_info_file}
len --> u64
num_terms --> u64
block_meta_file --> (on len bytes) 
term_info_file --> 
```



### Token positions in documents
[Ref](../../src/positions/serializer.rs)


Token positions are stored in three parts and over two files:
- File `.pos`: contains bitpacked of the positions delta
- File `.posidx`: contains bytes and long skip index


#### Position file
The position file (.pos) stores data for each field (it's a [composite file](#composite-file-structure)) and thus has the dedicated footer to get data for each field. The following data structure is repeated for each field, we omit that repetition for clarity.

```
Position file (.pos) := *DeltaPositionsForTerm* ^ *NumTerms*
```

The delta positions are the difference between two consecutive positions, and are encoded in blocks like the posting lists format.

```
* *DeltaPositionsForTerms* := *NumBitPackedBlocks* *BitPackedPositionBlock*^(P/128) *BitPackedPositionsDeltaBitWidth* *VIntPosDeltas*?
* *NumBitPackedBlocks**: := *P* / 128 encoded as a variable byte integer.
* *BitPackedPositionBlock* := bit width encoded block of 128 positions delta
* *BitPackedPositionsDeltaBitWidth* := (*BitWidth*: u8)^*NumBitPackedBlocks*
* *VIntPosDeltas* := *VIntPosDelta*^(*P* % 128).
```


### Doc store

### Fast fields

### Fieldnorm

### Delete documents



