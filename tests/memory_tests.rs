//! Memory regression tests.
//!
//! Uses a custom global allocator to track peak and current heap usage.
//! Each test measures the memory footprint of a key operation and asserts
//! that it stays within a defined budget.  If an operation suddenly uses
//! significantly more memory, the test will fail — catching regressions
//! before they reach production.
//!
//! Run with:
//!   cargo test --test memory_tests -- --test-threads=1
//!
//! The `--test-threads=1` flag is **required** because all tests share
//! one global allocator counter.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// Tracking allocator
// ---------------------------------------------------------------------------

struct TrackingAllocator;

static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: TrackingAllocator = TrackingAllocator;

unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            let current = ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            // Update peak (lock-free CAS loop)
            let mut peak = PEAK.load(Ordering::Relaxed);
            while current > peak {
                match PEAK.compare_exchange_weak(
                    peak,
                    current,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(actual) => peak = actual,
                }
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
}

fn reset_tracking() {
    let cur = ALLOCATED.load(Ordering::SeqCst);
    PEAK.store(cur, Ordering::SeqCst);
}

fn current_bytes() -> usize {
    ALLOCATED.load(Ordering::SeqCst)
}

fn peak_bytes() -> usize {
    PEAK.load(Ordering::SeqCst)
}

/// Serialize tests so the counters don't interleave.
static TEST_MUTEX: Mutex<()> = Mutex::new(());

/// Helper: measure peak delta and retained delta for a closure.
fn measure<F: FnOnce() -> R, R>(f: F) -> (R, MemStats) {
    let _guard = TEST_MUTEX.lock().unwrap();
    // Stabilize: force a collection point
    let before = current_bytes();
    reset_tracking();

    let result = f();

    let peak = peak_bytes();
    let after = current_bytes();

    let peak_delta = peak.saturating_sub(before);
    let retained = after.saturating_sub(before);

    (
        result,
        MemStats {
            peak_delta,
            retained,
        },
    )
}

#[derive(Debug)]
struct MemStats {
    /// Peak heap growth during the operation.
    peak_delta: usize,
    /// Heap still held after the operation (before dropping the result).
    retained: usize,
}

impl MemStats {
    fn peak_kb(&self) -> usize {
        self.peak_delta / 1024
    }
    fn retained_kb(&self) -> usize {
        self.retained / 1024
    }
}

// ---------------------------------------------------------------------------
// Imports
// ---------------------------------------------------------------------------

use ast_index::db;
use ast_index::parsers::treesitter::{
    cpp::CPP_PARSER, dart::DART_PARSER, go::GO_PARSER, java::JAVA_PARSER, kotlin::KOTLIN_PARSER,
    matlab::MATLAB_PARSER, python::PYTHON_PARSER, ruby::RUBY_PARSER, rust_lang::RUST_PARSER,
    scala::SCALA_PARSER, swift::SWIFT_PARSER, typescript::TYPESCRIPT_PARSER, LanguageParser,
};
use ast_index::parsers::{parse_file_symbols, FileType};
use rusqlite::{params, Connection};

// ---------------------------------------------------------------------------
// Code snippets (same as bench but kept self-contained)
// ---------------------------------------------------------------------------

const KOTLIN_CODE: &str = r#"
package com.example.payments

import kotlinx.coroutines.flow.Flow
import javax.inject.Inject

sealed class PaymentState {
    object Idle : PaymentState()
    data class Loading(val orderId: String) : PaymentState()
    data class Success(val receipt: Receipt) : PaymentState()
    data class Error(val message: String) : PaymentState()
}

data class Receipt(val id: String, val amount: Double, val currency: String)

interface PaymentRepository {
    suspend fun processPayment(orderId: String, amount: Double): Result<Receipt>
    suspend fun getHistory(userId: String): Flow<List<Receipt>>
    suspend fun refund(receiptId: String): Result<Unit>
}

class PaymentRepositoryImpl @Inject constructor(
    private val api: PaymentApi,
    private val cache: PaymentCache,
    private val analytics: AnalyticsTracker,
) : PaymentRepository {

    override suspend fun processPayment(orderId: String, amount: Double): Result<Receipt> {
        return try {
            val receipt = api.charge(orderId, amount)
            cache.save(receipt)
            analytics.track("payment_success", mapOf("order" to orderId))
            Result.success(receipt)
        } catch (e: Exception) {
            Result.failure(e)
        }
    }

    override suspend fun getHistory(userId: String): Flow<List<Receipt>> = cache.getHistory(userId)
    override suspend fun refund(receiptId: String): Result<Unit> = api.refund(receiptId).map { }

    companion object {
        const val MAX_RETRY = 3
        const val TIMEOUT_MS = 5000L
    }
}
"#;

const JAVA_CODE: &str = r#"
package com.example.users;

import org.springframework.web.bind.annotation.*;
import org.springframework.http.ResponseEntity;
import java.util.List;
import java.util.Optional;

@RestController
@RequestMapping("/api/v1/users")
public class UserController {
    private final UserService userService;

    public UserController(UserService userService) {
        this.userService = userService;
    }

    @GetMapping
    public ResponseEntity<List<UserDto>> getAll(@RequestParam(defaultValue = "0") int page) {
        return ResponseEntity.ok(userService.findAll(page, 20));
    }

    @GetMapping("/{id}")
    public ResponseEntity<UserDto> getById(@PathVariable Long id) {
        return userService.findById(id)
                .map(ResponseEntity::ok)
                .orElse(ResponseEntity.notFound().build());
    }

    @PostMapping
    public ResponseEntity<UserDto> create(@RequestBody CreateUserRequest request) {
        return ResponseEntity.status(201).body(userService.create(request));
    }

    @DeleteMapping("/{id}")
    public ResponseEntity<Void> delete(@PathVariable Long id) {
        userService.delete(id);
        return ResponseEntity.noContent().build();
    }
}

interface UserService {
    List<UserDto> findAll(int page, int size);
    Optional<UserDto> findById(Long id);
    UserDto create(CreateUserRequest request);
    void delete(Long id);
}
"#;

const SWIFT_CODE: &str = r#"
import Foundation
import Combine

protocol NetworkService {
    func fetch<T: Decodable>(url: URL) async throws -> T
}

class APIClient: NetworkService {
    private let session: URLSession

    init(session: URLSession = .shared) { self.session = session }

    func fetch<T: Decodable>(url: URL) async throws -> T {
        let (data, _) = try await session.data(from: url)
        return try JSONDecoder().decode(T.self, from: data)
    }
}

extension APIClient {
    func fetchUsers() async throws -> [User] {
        try await fetch(url: URL(string: "https://api.example.com/users")!)
    }
}

struct User: Codable, Identifiable {
    let id: UUID
    let name: String
    let email: String
}

enum NetworkError: Error { case invalidResponse, unauthorized }
"#;

const TYPESCRIPT_CODE: &str = r#"
interface User { id: string; name: string; email: string; }
interface UserListProps { initialPage?: number; pageSize?: number; }

class UserAPI {
    private baseUrl: string;
    constructor(baseUrl: string) { this.baseUrl = baseUrl; }

    async getUsers(page: number, size: number): Promise<User[]> {
        const res = await fetch(`${this.baseUrl}/users?page=${page}&size=${size}`);
        return res.json();
    }
}

export default class UserList {}
export { UserAPI };
export type { User };
"#;

const PYTHON_CODE: &str = r#"
from dataclasses import dataclass, field
from typing import Optional, List, Dict, Any
from abc import ABC, abstractmethod

@dataclass
class Config:
    host: str = "localhost"
    port: int = 8080
    debug: bool = False

class BaseHandler(ABC):
    def __init__(self, config: Config):
        self.config = config

    @abstractmethod
    def handle(self, request: Dict[str, Any]) -> Dict[str, Any]:
        pass

class RequestHandler(BaseHandler):
    def handle(self, request: Dict[str, Any]) -> Dict[str, Any]:
        return {"status": "ok", "data": request.get("payload")}

def create_app(config: Config) -> RequestHandler:
    return RequestHandler(config)
"#;

/// ~300-line Kotlin file for large-file memory tests.
const LARGE_KOTLIN_CODE: &str = r#"
package com.example.feature.checkout

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.map
import javax.inject.Inject

sealed class CheckoutEvent {
    data class AddItem(val productId: String, val quantity: Int) : CheckoutEvent()
    data class RemoveItem(val productId: String) : CheckoutEvent()
    data class UpdateQuantity(val productId: String, val quantity: Int) : CheckoutEvent()
    data class ApplyPromo(val code: String) : CheckoutEvent()
    object ClearCart : CheckoutEvent()
    data class SetShipping(val address: ShippingAddress) : CheckoutEvent()
    data class SetPayment(val method: PaymentMethod) : CheckoutEvent()
    object PlaceOrder : CheckoutEvent()
}

sealed class CheckoutState {
    object Empty : CheckoutState()
    data class Active(
        val items: List<CartItem>,
        val subtotal: Double,
        val discount: Double,
        val shipping: Double,
        val total: Double,
        val promoCode: String? = null,
        val shippingAddress: ShippingAddress? = null,
        val paymentMethod: PaymentMethod? = null,
    ) : CheckoutState()
    data class Processing(val orderId: String) : CheckoutState()
    data class Completed(val orderId: String, val receipt: OrderReceipt) : CheckoutState()
    data class Failed(val error: String) : CheckoutState()
}

data class CartItem(
    val productId: String,
    val name: String,
    val price: Double,
    val quantity: Int,
    val imageUrl: String,
) {
    val lineTotal: Double get() = price * quantity
}

data class ShippingAddress(
    val name: String, val street: String, val city: String,
    val state: String, val zipCode: String, val country: String,
)

sealed class PaymentMethod {
    data class CreditCard(val last4: String, val brand: String, val token: String) : PaymentMethod()
    data class PayPal(val email: String) : PaymentMethod()
    object ApplePay : PaymentMethod()
    object GooglePay : PaymentMethod()
}

data class OrderReceipt(val orderId: String, val items: List<CartItem>, val total: Double, val timestamp: Long)

interface CheckoutRepository {
    suspend fun calculateShipping(address: ShippingAddress, items: List<CartItem>): Double
    suspend fun validatePromo(code: String): PromoResult
    suspend fun placeOrder(state: CheckoutState.Active): Result<OrderReceipt>
    fun getCartItems(): Flow<List<CartItem>>
}

sealed class PromoResult {
    data class Valid(val discount: Double, val description: String) : PromoResult()
    data class Invalid(val reason: String) : PromoResult()
    data class Expired(val expiredAt: Long) : PromoResult()
}

class CheckoutViewModel @Inject constructor(
    private val repository: CheckoutRepository,
    private val analytics: AnalyticsTracker,
    private val logger: AppLogger,
) {
    private val _state = MutableStateFlow<CheckoutState>(CheckoutState.Empty)
    val state: StateFlow<CheckoutState> = _state.asStateFlow()

    val itemCount: Flow<Int> = _state.map { st ->
        when (st) {
            is CheckoutState.Active -> st.items.sumOf { it.quantity }
            else -> 0
        }
    }

    suspend fun onEvent(event: CheckoutEvent) {
        when (event) {
            is CheckoutEvent.AddItem -> addItem(event.productId, event.quantity)
            is CheckoutEvent.RemoveItem -> removeItem(event.productId)
            is CheckoutEvent.UpdateQuantity -> updateQuantity(event.productId, event.quantity)
            is CheckoutEvent.ApplyPromo -> applyPromo(event.code)
            is CheckoutEvent.ClearCart -> clearCart()
            is CheckoutEvent.SetShipping -> setShipping(event.address)
            is CheckoutEvent.SetPayment -> setPayment(event.method)
            is CheckoutEvent.PlaceOrder -> placeOrder()
        }
    }

    private suspend fun addItem(productId: String, quantity: Int) {
        val current = _state.value
        logger.debug("Adding item $productId x$quantity")
        analytics.track("cart_add", mapOf("product" to productId, "qty" to quantity.toString()))
    }

    private fun removeItem(productId: String) {
        val current = _state.value as? CheckoutState.Active ?: return
        val newItems = current.items.filter { it.productId != productId }
        if (newItems.isEmpty()) _state.value = CheckoutState.Empty
        else _state.value = recalculate(current.copy(items = newItems))
    }

    private fun updateQuantity(productId: String, quantity: Int) {
        val current = _state.value as? CheckoutState.Active ?: return
        if (quantity <= 0) { removeItem(productId); return }
        val newItems = current.items.map {
            if (it.productId == productId) it.copy(quantity = quantity) else it
        }
        _state.value = recalculate(current.copy(items = newItems))
    }

    private suspend fun applyPromo(code: String) {
        val current = _state.value as? CheckoutState.Active ?: return
        when (val result = repository.validatePromo(code)) {
            is PromoResult.Valid -> {
                _state.value = recalculate(current.copy(promoCode = code, discount = result.discount))
            }
            is PromoResult.Invalid -> logger.warn("Invalid promo: ${result.reason}")
            is PromoResult.Expired -> logger.warn("Promo expired at ${result.expiredAt}")
        }
    }

    private fun clearCart() { _state.value = CheckoutState.Empty }

    private suspend fun setShipping(address: ShippingAddress) {
        val current = _state.value as? CheckoutState.Active ?: return
        val shippingCost = repository.calculateShipping(address, current.items)
        _state.value = recalculate(current.copy(shippingAddress = address, shipping = shippingCost))
    }

    private fun setPayment(method: PaymentMethod) {
        val current = _state.value as? CheckoutState.Active ?: return
        _state.value = current.copy(paymentMethod = method)
    }

    private suspend fun placeOrder() {
        val current = _state.value as? CheckoutState.Active ?: return
        _state.value = CheckoutState.Processing("pending")
        val result = repository.placeOrder(current)
        result.fold(
            onSuccess = { receipt ->
                _state.value = CheckoutState.Completed(receipt.orderId, receipt)
            },
            onFailure = { error ->
                _state.value = CheckoutState.Failed(error.message ?: "Order failed")
            }
        )
    }

    private fun recalculate(state: CheckoutState.Active): CheckoutState.Active {
        val subtotal = state.items.sumOf { it.lineTotal }
        val total = subtotal - state.discount + state.shipping
        return state.copy(subtotal = subtotal, total = total)
    }

    companion object {
        const val MAX_ITEMS = 99
        const val FREE_SHIPPING_THRESHOLD = 50.0
    }
}

interface AnalyticsTracker { fun track(event: String, params: Map<String, String>) }
interface AppLogger {
    fun debug(msg: String)
    fun warn(msg: String)
    fun error(msg: String, throwable: Throwable? = null)
}
"#;

// ===========================================================================
// Parser memory tests
// ===========================================================================

/// Budget per small file (~50 lines): 512 KB peak.
const PARSER_SMALL_BUDGET: usize = 512 * 1024;
/// Budget per large file (~300 lines): 2 MB peak.
const PARSER_LARGE_BUDGET: usize = 2 * 1024 * 1024;

macro_rules! parser_memory_test {
    ($name:ident, $parser:expr, $code:expr, $budget:expr) => {
        #[test]
        fn $name() {
            let (symbols, stats) = measure(|| $parser.parse_symbols($code).unwrap());

            eprintln!(
                "[{}] symbols={}, peak={}KB, retained={}KB",
                stringify!($name),
                symbols.len(),
                stats.peak_kb(),
                stats.retained_kb(),
            );

            assert!(
                stats.peak_delta < $budget,
                "{}: peak {}KB exceeds budget {}KB",
                stringify!($name),
                stats.peak_kb(),
                $budget / 1024,
            );

            // After dropping the result, almost nothing should remain.
            drop(symbols);
        }
    };
}

parser_memory_test!(
    parser_memory_kotlin,
    KOTLIN_PARSER,
    KOTLIN_CODE,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_java,
    JAVA_PARSER,
    JAVA_CODE,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_swift,
    SWIFT_PARSER,
    SWIFT_CODE,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_typescript,
    TYPESCRIPT_PARSER,
    TYPESCRIPT_CODE,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_python,
    PYTHON_PARSER,
    PYTHON_CODE,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(parser_memory_go, GO_PARSER, GO_SNIPPET, PARSER_SMALL_BUDGET);
parser_memory_test!(
    parser_memory_rust,
    RUST_PARSER,
    RUST_SNIPPET,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_cpp,
    CPP_PARSER,
    CPP_SNIPPET,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_ruby,
    RUBY_PARSER,
    RUBY_SNIPPET,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_dart,
    DART_PARSER,
    DART_SNIPPET,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_scala,
    SCALA_PARSER,
    SCALA_SNIPPET,
    PARSER_SMALL_BUDGET
);
parser_memory_test!(
    parser_memory_matlab,
    MATLAB_PARSER,
    MATLAB_CODE,
    PARSER_SMALL_BUDGET
);

// Large files
parser_memory_test!(
    parser_memory_kotlin_large,
    KOTLIN_PARSER,
    LARGE_KOTLIN_CODE,
    PARSER_LARGE_BUDGET
);

// Go, Rust, C++, Ruby, Dart, Scala snippets (reuse from inline — small)
const GO_SNIPPET: &str = r#"
package server

type Server struct {
    handlers map[string]Handler
    config   *Config
}

type Config struct {
    Addr string
    MaxBodySize int64
}

type Handler interface {
    ServeHTTP(w http.ResponseWriter, r *http.Request)
    Pattern() string
}

func NewServer(cfg *Config) *Server {
    return &Server{handlers: make(map[string]Handler), config: cfg}
}

func (s *Server) Register(h Handler) { s.handlers[h.Pattern()] = h }
"#;

const RUST_SNIPPET: &str = r#"
use std::collections::HashMap;

pub trait Storage: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: Vec<u8>) -> Result<(), StorageError>;
}

pub enum StorageError {
    NotFound(String),
    Io(std::io::Error),
}

pub struct InMemoryStorage {
    data: HashMap<String, Vec<u8>>,
}

impl InMemoryStorage {
    pub fn new() -> Self { Self { data: HashMap::new() } }
}
"#;

const CPP_SNIPPET: &str = r#"
#include <memory>
#include <string>
#include <vector>

namespace engine {

class Texture {
public:
    Texture(int width, int height);
    int width() const;
    int height() const;
private:
    int width_;
    int height_;
};

class Material {
public:
    Material(const std::string& name);
    void setTexture(const std::string& slot, std::shared_ptr<Texture> tex);
private:
    std::string name_;
};

class Renderer {
public:
    virtual void draw(const Material& mat, const std::vector<float>& verts);
};

} // namespace engine
"#;

const RUBY_SNIPPET: &str = r#"
module Authentication
  class TokenService
    attr_reader :secret, :expiry

    def initialize(secret:, expiry: 3600)
      @secret = secret
      @expiry = expiry
    end

    def generate(user)
      { user_id: user.id, exp: Time.now.to_i + expiry }.to_json
    end

    def verify(token)
      JSON.parse(token, symbolize_names: true)
    end
  end
end

class User
  attr_accessor :id, :email, :name
  def initialize(id:, email:, name:); @id = id; @email = email; @name = name; end
end
"#;

const DART_SNIPPET: &str = r#"
abstract class AppEvent {}
class LoadDataEvent extends AppEvent { final String query; LoadDataEvent(this.query); }
class RefreshEvent extends AppEvent {}

abstract class AppState {}
class InitialState extends AppState {}
class LoadingState extends AppState {}
class LoadedState extends AppState { final List<Item> items; LoadedState(this.items); }

class Item {
  final String id;
  final String title;
  final double price;
  const Item({required this.id, required this.title, required this.price});
}

mixin LoggerMixin { void log(String message) {} }

extension StringExt on String {
  String capitalize() => isEmpty ? this : '${this[0].toUpperCase()}${substring(1)}';
}
"#;

const SCALA_SNIPPET: &str = r#"
package com.example.catalog

case class Product(id: String, name: String, price: BigDecimal, category: String)

trait CatalogRepository {
  def findById(id: String): Option[Product]
  def findByCategory(category: String): List[Product]
  def save(product: Product): Either[String, Product]
}

object CatalogRepository {
  def apply(): CatalogRepository = new InMemoryCatalog()
}

class InMemoryCatalog extends CatalogRepository {
  private var store: Map[String, Product] = Map.empty
  override def findById(id: String): Option[Product] = store.get(id)
  override def findByCategory(cat: String): List[Product] = store.values.filter(_.category == cat).toList
  override def save(p: Product): Either[String, Product] = { store += (p.id -> p); Right(p) }
}
"#;

const MATLAB_CODE: &str = r#"
classdef Vehicle < handle
    properties
        Make
        Model
        Year
    end
    methods
        function obj = Vehicle(make, model, year)
            obj.Make = make;
            obj.Model = model;
            obj.Year = year;
        end
        function display(obj)
            fprintf('%s %s %d\n', obj.Make, obj.Model, obj.Year);
        end
    end
    enumeration
        Car, Truck, SUV
    end
end
"#;

// ===========================================================================
// Full pipeline memory test (parse → DB write)
// ===========================================================================

/// Budget for full pipeline on a single ~50-line file: 1 MB peak.
const PIPELINE_SINGLE_BUDGET: usize = 1024 * 1024;

#[test]
fn pipeline_memory_single_file() {
    let (_, stats) = measure(|| {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn).unwrap();

        let (symbols, refs) = parse_file_symbols(KOTLIN_CODE, FileType::Kotlin).unwrap();

        conn.execute(
            "INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)",
            params!["src/Payment.kt", 1000i64, 500i64],
        )
        .unwrap();
        let file_id = conn.last_insert_rowid();

        for sym in &symbols {
            conn.execute(
                "INSERT INTO symbols (file_id, name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![file_id, sym.name, sym.kind.as_str(), sym.line as i64, sym.signature],
            )
            .unwrap();
            let sym_id = conn.last_insert_rowid();
            for (parent, kind) in &sym.parents {
                conn.execute(
                    "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)",
                    params![sym_id, parent, kind],
                )
                .unwrap();
            }
        }

        for r in &refs {
            conn.execute(
                "INSERT INTO refs (file_id, name, line, context) VALUES (?1, ?2, ?3, ?4)",
                params![file_id, r.name, r.line as i64, r.context],
            )
            .unwrap();
        }

        (symbols.len(), refs.len())
    });

    eprintln!(
        "[pipeline_single_file] peak={}KB, retained={}KB",
        stats.peak_kb(),
        stats.retained_kb(),
    );

    assert!(
        stats.peak_delta < PIPELINE_SINGLE_BUDGET,
        "Pipeline peak {}KB exceeds budget {}KB",
        stats.peak_kb(),
        PIPELINE_SINGLE_BUDGET / 1024,
    );
}

// ===========================================================================
// DB memory tests
// ===========================================================================

/// Budget for a 10K-symbol DB creation: 32 MB peak.
const DB_10K_BUDGET: usize = 32 * 1024 * 1024;
/// Budget for a single FTS5 search on 10K DB: 512 KB peak.
const SEARCH_BUDGET: usize = 512 * 1024;

fn create_10k_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_db(&conn).unwrap();

    let kinds = ["class", "interface", "function", "property", "enum"];

    conn.execute_batch("BEGIN").unwrap();
    for i in 0..10_000 {
        let path = format!("src/mod{}/File{}.kt", i / 100, i);
        conn.execute(
            "INSERT OR REPLACE INTO files (path, mtime, size) VALUES (?1, ?2, ?3)",
            params![path, 1000i64, 500i64],
        )
        .unwrap();
        let fid = conn.last_insert_rowid();

        let name = format!("Symbol{}", i);
        let kind = kinds[i % kinds.len()];
        conn.execute(
            "INSERT INTO symbols (file_id, name, kind, line, signature) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![fid, name, kind, (i + 1) as i64, format!("{} {}", kind, name)],
        )
        .unwrap();
        let sid = conn.last_insert_rowid();

        if kind == "class" || kind == "interface" {
            for parent in &["BaseClass", "Serializable", "Comparable"] {
                conn.execute(
                    "INSERT INTO inheritance (child_id, parent_name, kind) VALUES (?1, ?2, ?3)",
                    params![sid, parent, "extends"],
                )
                .unwrap();
            }
        }

        for r in 0..3 {
            let rname = format!("Symbol{}", (i + r * 7) % 10_000);
            conn.execute(
                "INSERT INTO refs (file_id, name, line, context) VALUES (?1, ?2, ?3, ?4)",
                params![
                    fid,
                    rname,
                    (r * 10) as i64,
                    format!("val x = {}.do()", rname)
                ],
            )
            .unwrap();
        }
    }
    conn.execute_batch("COMMIT").unwrap();

    conn
}

#[test]
fn db_memory_create_10k() {
    let (conn, stats) = measure(create_10k_db);

    eprintln!(
        "[db_create_10k] peak={}KB, retained={}KB",
        stats.peak_kb(),
        stats.retained_kb(),
    );

    assert!(
        stats.peak_delta < DB_10K_BUDGET,
        "DB create peak {}KB exceeds budget {}KB",
        stats.peak_kb(),
        DB_10K_BUDGET / 1024,
    );

    drop(conn);
}

#[test]
fn db_memory_fts5_search() {
    // Setup outside measurement
    let conn = create_10k_db();

    let (results, stats) = measure(|| db::search_symbols(&conn, "Symbol5000", 20).unwrap());

    eprintln!(
        "[db_fts5_search] results={}, peak={}KB, retained={}KB",
        results.len(),
        stats.peak_kb(),
        stats.retained_kb(),
    );

    assert!(
        stats.peak_delta < SEARCH_BUDGET,
        "FTS5 search peak {}KB exceeds budget {}KB",
        stats.peak_kb(),
        SEARCH_BUDGET / 1024,
    );
}

#[test]
fn db_memory_fuzzy_search() {
    let conn = create_10k_db();

    let (results, stats) = measure(|| db::search_symbols_fuzzy(&conn, "Symbo", 20).unwrap());

    eprintln!(
        "[db_fuzzy_search] results={}, peak={}KB, retained={}KB",
        results.len(),
        stats.peak_kb(),
        stats.retained_kb(),
    );

    // Fuzzy scans more data → 2 MB budget
    assert!(
        stats.peak_delta < 2 * 1024 * 1024,
        "Fuzzy search peak {}KB exceeds budget 2048KB",
        stats.peak_kb(),
    );
}

#[test]
fn db_memory_find_implementations() {
    let conn = create_10k_db();

    let (results, stats) = measure(|| db::find_implementations(&conn, "BaseClass", 100).unwrap());

    eprintln!(
        "[db_find_implementations] results={}, peak={}KB, retained={}KB",
        results.len(),
        stats.peak_kb(),
        stats.retained_kb(),
    );

    assert!(
        stats.peak_delta < SEARCH_BUDGET,
        "find_implementations peak {}KB exceeds budget {}KB",
        stats.peak_kb(),
        SEARCH_BUDGET / 1024,
    );
}

#[test]
fn db_memory_find_references() {
    let conn = create_10k_db();

    let (results, stats) = measure(|| db::find_references(&conn, "Symbol500", 100).unwrap());

    eprintln!(
        "[db_find_references] results={}, peak={}KB, retained={}KB",
        results.len(),
        stats.peak_kb(),
        stats.retained_kb(),
    );

    assert!(
        stats.peak_delta < SEARCH_BUDGET,
        "find_references peak {}KB exceeds budget {}KB",
        stats.peak_kb(),
        SEARCH_BUDGET / 1024,
    );
}

// ===========================================================================
// Leak detection test — parse many times, ensure no growth
// ===========================================================================

#[test]
fn parser_no_leak_100_iterations() {
    let (_result, stats) = measure(|| {
        let mut total_symbols = 0usize;
        for _ in 0..100 {
            let syms = KOTLIN_PARSER.parse_symbols(KOTLIN_CODE).unwrap();
            total_symbols += syms.len();
            // syms dropped here each iteration
        }
        total_symbols
    });

    eprintln!(
        "[parser_no_leak_100] peak={}KB, retained={}KB",
        stats.peak_kb(),
        stats.retained_kb(),
    );

    // After 100 iterations, retained should be negligible
    // (only the counter + internal parser state reuse).
    // Allow up to 256 KB retained — in practice should be near 0.
    assert!(
        stats.retained < 256 * 1024,
        "After 100 parses, {}KB still retained — possible leak",
        stats.retained_kb(),
    );
}
