import XCTest
import SwiftTreeSitter
import TreeSitterRedis

final class TreeSitterRedisTests: XCTestCase {
    func testCanLoadGrammar() throws {
        let parser = Parser()
        let language = Language(language: tree_sitter_redis())
        XCTAssertNoThrow(try parser.setLanguage(language),
                         "Error loading Redis grammar")
    }
}
